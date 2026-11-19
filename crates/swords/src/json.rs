//! A JSON value that keeps the order of an object's fields.
//!
//! The site's replies and this command's `--json` output are printed in the order they were made.
//! `serde_json::Value` sorts fields unless a crate-wide feature is on, and that feature would also
//! change any program that compiles this crate in. So this small type does the job here.

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// One JSON value. An object is a list of fields, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Make an object from fields, in the order given.
    pub fn object<const N: usize>(fields: [(&str, Json); N]) -> Json {
        Json::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Read JSON text. Text that is not JSON gives an empty object, as a reply with no body does.
    pub fn parse(text: &[u8]) -> Json {
        serde_json::from_slice(text).unwrap_or(Json::Object(Vec::new()))
    }

    /// A field of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Set a field of an object: replace it in place, or add it at the end.
    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Object(fields) = self {
            match fields.iter_mut().find(|(k, _)| k == key) {
                Some(field) => field.1 = value,
                None => fields.push((key.to_string(), value)),
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    /// A field as text for a message: a string as it is, a number as digits, anything else empty.
    pub fn text(&self, key: &str) -> String {
        match self.get(key) {
            Some(Json::String(s)) => s.clone(),
            Some(Json::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }

    /// A field, or null when it is not there.
    pub fn field(&self, key: &str) -> Json {
        self.get(key).cloned().unwrap_or(Json::Null)
    }

    /// The value as indented JSON text (two spaces), with no newline at the end.
    pub fn pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The same value as a `serde_json::Value`, for the state file.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::String(s.to_string())
    }
}
impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::String(s)
    }
}
impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}
impl From<Option<&str>> for Json {
    fn from(s: Option<&str>) -> Json {
        s.map_or(Json::Null, Json::from)
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => out.serialize_unit(),
            Json::Bool(b) => out.serialize_bool(*b),
            Json::Number(n) => n.serialize(out),
            Json::String(s) => out.serialize_str(s),
            Json::Array(items) => {
                let mut seq = out.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(fields) => {
                let mut map = out.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Json, D::Error> {
        struct Reader;
        impl<'de> Visitor<'de> for Reader {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
                Ok(Json::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_f64<E>(self, n: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(n).map_or(Json::Null, Json::Number))
            }
            fn visit_str<E>(self, s: &str) -> Result<Json, E> {
                Ok(Json::String(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<Json, E> {
                Ok(Json::String(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry::<String, Json>()? {
                    fields.push(field);
                }
                Ok(Json::Object(fields))
            }
        }
        input.deserialize_any(Reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_field_order() {
        let text = "{\n  \"title\": \"Sample\",\n  \"author\": \"A\",\n  \"grid\": [\n    \"AB\",\n    \"CD\"\n  ],\n  \"n\": 1.5,\n  \"none\": null\n}";
        assert_eq!(Json::parse(text.as_bytes()).pretty(), text);
    }

    #[test]
    fn set_replaces_in_place_or_adds_at_the_end() {
        let mut j = Json::object([("a", "1".into()), ("b", "2".into())]);
        j.set("a", true.into());
        j.set("c", Json::Null);
        assert_eq!(serde_json::to_string(&j).unwrap(), r#"{"a":true,"b":"2","c":null}"#);
    }

    #[test]
    fn text_that_is_not_json_is_an_empty_object() {
        assert_eq!(Json::parse(b"<html>"), Json::Object(vec![]));
    }

    #[test]
    fn text_of_a_field() {
        let j = Json::parse(br#"{"t":"x","n":3,"z":null}"#);
        assert_eq!((j.text("t"), j.text("n"), j.text("z"), j.text("no")), ("x".into(), "3".into(), "".into(), "".into()));
    }
}
