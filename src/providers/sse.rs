//! SSE(Server-Sent Events)增量解析器。
//!
//! 处理:网络块边界任意切分、UTF-8 多字节字符跨块、事件边界
//! (`\n\n` / `\r\n\r\n`)、字段(`data:`/`event:`/`id:`/注释)。
//! 对 chat/completions 的 JSON data 逐事件交给上层;`: OPENROUTER PROCESSING`
//! 等注释事件原样透传(上层视为 keep-alive,不算断流)。

#[derive(Debug, Clone, PartialEq)]
pub enum SseMessage {
    /// data 字段(多个 data 行已按 SSE 规范以 \n 连接)。
    Data(String),
    /// 注释行(以 : 开头)。
    Comment(String),
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// data 行累积。
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个网络块,返回完整事件。块可以在任意字节边界切分
    /// (UTF-8 序列中间也可以:只有完整行才会被解码)。
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseMessage> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        // 找行尾(\n 或 \r\n);行未完整则留待下一块
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line_end = nl;
            let mut next_start = nl + 1;
            if nl > 0 && self.buf[nl - 1] == b'\r' {
                line_end = nl - 1;
            }
            let line = String::from_utf8_lossy(&self.buf[..line_end]).into_owned();
            self.buf.drain(..next_start);
            let _ = &mut next_start;

            if line.is_empty() {
                // 事件边界
                if !self.data_lines.is_empty() {
                    out.push(SseMessage::Data(self.data_lines.join("\n")));
                    self.data_lines.clear();
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix(':') {
                out.push(SseMessage::Comment(rest.trim().to_string()));
                continue;
            }
            // field: value(冒号后可选一个空格)
            if let Some(colon) = line.find(':') {
                let field = &line[..colon];
                let mut value = &line[colon + 1..];
                if value.starts_with(' ') {
                    value = &value[1..];
                }
                if field == "data" {
                    self.data_lines.push(value.to_string());
                }
                // event:/id:/retry: 首版不消费
            } else {
                // 无值字段行(如 "data")
                if line == "data" {
                    self.data_lines.push(String::new());
                }
            }
        }
        out
    }

    /// 流结束:若缓冲里还有未以空行收尾的 data,补发。
    pub fn finish(&mut self) -> Vec<SseMessage> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            // 尾行无换行
            let line = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            self.apply_line(line, &mut out);
        }
        if !self.data_lines.is_empty() {
            out.push(SseMessage::Data(self.data_lines.join("\n")));
            self.data_lines.clear();
        }
        out
    }

    fn apply_line(&mut self, line: String, out: &mut Vec<SseMessage>) {
        if line.is_empty() {
            if !self.data_lines.is_empty() {
                out.push(SseMessage::Data(self.data_lines.join("\n")));
                self.data_lines.clear();
            }
            return;
        }
        if let Some(rest) = line.strip_prefix(':') {
            out.push(SseMessage::Comment(rest.trim().to_string()));
            return;
        }
        if let Some(colon) = line.find(':') {
            let field = &line[..colon];
            let mut value = &line[colon + 1..];
            if value.starts_with(' ') {
                value = &value[1..];
            }
            if field == "data" {
                self.data_lines.push(value.to_string());
            }
        } else if line == "data" {
            self.data_lines.push(String::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_events() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: {\"a\":1}\n\ndata: {\"a\":2}\n\n");
        assert_eq!(
            evs,
            vec![
                SseMessage::Data("{\"a\":1}".into()),
                SseMessage::Data("{\"a\":2}".into())
            ]
        );
    }

    #[test]
    fn split_across_chunks_mid_utf8() {
        let mut p = SseParser::new();
        let payload = "data: {\"text\":\"中文内容\"}\n\n".as_bytes().to_vec();
        // 逐字节喂入:UTF-8 与行边界都被切碎
        let mut got = Vec::new();
        for b in payload {
            got.extend(p.feed(&[b]));
        }
        assert_eq!(
            got,
            vec![SseMessage::Data("{\"text\":\"中文内容\"}".into())]
        );
    }

    #[test]
    fn crlf_and_multiline_data() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: line1\r\ndata: line2\r\n\r\n");
        assert_eq!(evs, vec![SseMessage::Data("line1\nline2".into())]);
    }

    #[test]
    fn comment_passthrough() {
        let mut p = SseParser::new();
        let evs = p.feed(b": OPENROUTER PROCESSING\n\ndata: {}\n\n");
        assert_eq!(
            evs,
            vec![
                SseMessage::Comment("OPENROUTER PROCESSING".into()),
                SseMessage::Data("{}".into())
            ]
        );
    }

    #[test]
    fn finish_flushes_partial() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: tail");
        assert!(evs.is_empty());
        assert_eq!(p.finish(), vec![SseMessage::Data("tail".into())]);
    }

    #[test]
    fn no_colon_data_field() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data\n\n");
        assert_eq!(evs, vec![SseMessage::Data(String::new())]);
    }
}
