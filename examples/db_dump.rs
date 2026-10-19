//! 临时工具:打印真实数据库内容(验收用)。

fn main() {
    let path = aicouncil::storage::repo::db_path().expect("数据目录");
    let db = aicouncil::storage::open(&path).expect("open");
    let convs = db.conversations().unwrap_or_default();
    for c in &convs {
        println!(
            "conv {} main_leaf={:?}",
            c.id,
            c.main_leaf_id.map(|m| m.as_str())
        );
        let msgs = db.messages_of(c.id).unwrap_or_default();
        for m in &msgs {
            println!(
                "  {} parent={:?} role={} content={:?}",
                m.id.as_str(),
                m.parent_id.map(|p| p.as_str()),
                m.role.as_str(),
                if m.content.len() > 40 {
                    &m.content[..40]
                } else {
                    &m.content
                }
            );
        }
    }
}
