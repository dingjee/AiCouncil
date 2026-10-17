//! 强类型 ID(内部为 UUID v4 字符串,SQLite 存 TEXT)。
//!
//! newtype 防止 ID 混用;`Unassigned` 用于发送前尚未分配 ID 的占位。

use uuid::Uuid;

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_uuid_str(s: &str) -> Self {
                Uuid::parse_str(s)
                    .map(Self)
                    .unwrap_or_else(|_| Self(Uuid::nil()))
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }

            pub fn as_str(&self) -> String {
                self.0.to_string()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Uuid::parse_str(&s)
                    .map(Self)
                    .map_err(|e| serde::de::Error::custom(e))
            }
        }
    };
}

define_id!(GroupId, "分组 ID");
define_id!(ConversationId, "会话 ID");
define_id!(MessageId, "消息节点 ID");
define_id!(BatchId, "一次点击发送对应的批次 ID");
define_id!(GenerationId, "本地生成请求 ID");
define_id!(ProviderId, "提供方配置 ID");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip_serde() {
        let id = ConversationId::new();
        let s = serde_json::to_string(&id).unwrap();
        let back: ConversationId = serde_json::from_str(&s).unwrap();
        assert_eq!(id, back);
    }
}
