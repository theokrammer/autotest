use serde::{Deserialize, Serialize};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
    Null,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    pub params: T,
}

impl<T> JsonRpcRequest<T> {
    pub fn new(id: RequestId, method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification<T> {
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

impl<T> JsonRpcNotification<T> {
    pub fn new(method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorObject>,
}

impl<T> JsonRpcResponse<T> {
    pub fn success(id: RequestId, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: RequestId, code: i64, message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcErrorObject {
                code,
                message: message.into(),
                data,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractorRequestParams {
    pub file_path: String,
    pub source_content: String,
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractorResponsePayload {
    pub file: String,
    pub impl_hash: String,
    pub public_contract_hash: String,
    pub dependencies: std::collections::BTreeMap<String, String>,
    pub targets: Vec<serde_json::Value>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = JsonRpcRequest::new(RequestId::Number(1), "extract", serde_json::json!({"path": "foo.php"}));
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains(r#""jsonrpc":"2.0""#));
        assert!(serialized.contains(r#""id":1"#));
        assert!(serialized.contains(r#""method":"extract""#));

        let deserialized: JsonRpcRequest<serde_json::Value> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.id, RequestId::Number(1));
        assert_eq!(deserialized.method, "extract");
    }

    #[test]
    fn test_null_request_id_in_error() {
        let resp: JsonRpcResponse<()> = JsonRpcResponse::error(RequestId::Null, PARSE_ERROR, "Parse error", None);
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains(r#""id":null"#));
        assert!(serialized.contains(r#""code":-32700"#));

        let deserialized: JsonRpcResponse<serde_json::Value> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.id, RequestId::Null);
        assert_eq!(deserialized.error.unwrap().code, PARSE_ERROR);
    }
}
