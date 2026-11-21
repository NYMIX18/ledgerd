//! Safaricom Daraja (M-Pesa Express / STK push) callback helpers.
//!
//! NOTE: Daraja does **not** sign callbacks. The usual defences are (1) an unguessable
//! callback URL, (2) IP allow-listing at your proxy, and (3) never trusting the callback
//! alone: we also match it against a payment intent *we* created and compare the amount.
use serde_json::Value;
use subtle::ConstantTimeEq;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("missing field {0}")]
    Missing(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StkCallback {
    pub merchant_request_id: String,
    pub checkout_request_id: String,
    pub result_code: i32,
    pub result_desc: String,
    /// Amount converted to minor units (KES cents). `None` for failed payments.
    pub amount_minor: Option<i64>,
    pub receipt: Option<String>,
}

pub fn parse_stk_callback(v: &Value) -> Result<StkCallback, ParseError> {
    let cb = v
        .pointer("/Body/stkCallback")
        .ok_or(ParseError::Missing("Body.stkCallback"))?;
    let s = |k: &'static str| -> Result<String, ParseError> {
        cb.get(k)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(ParseError::Missing(k))
    };
    let result_code = cb
        .get("ResultCode")
        .and_then(Value::as_i64)
        .ok_or(ParseError::Missing("ResultCode"))? as i32;

    let mut amount_minor = None;
    let mut receipt = None;
    if let Some(items) = cb.pointer("/CallbackMetadata/Item").and_then(Value::as_array) {
        for item in items {
            match item.get("Name").and_then(Value::as_str) {
                Some("Amount") => {
                    // Daraja sends whole shillings as a JSON number (sometimes `1.00`).
                    // This is the one place a float appears: converting an *external* JSON
                    // number into integer minor units, rounded.
                    amount_minor = item
                        .get("Value")
                        .and_then(Value::as_f64)
                        .map(|k| (k * 100.0).round() as i64);
                }
                Some("MpesaReceiptNumber") => {
                    receipt = item.get("Value").and_then(Value::as_str).map(str::to_owned);
                }
                _ => {}
            }
        }
    }
    Ok(StkCallback {
        merchant_request_id: s("MerchantRequestID")?,
        checkout_request_id: s("CheckoutRequestID")?,
        result_code,
        result_desc: s("ResultDesc")?,
        amount_minor,
        receipt,
    })
}

/// Constant-time comparison of the secret path token.
pub fn token_matches(provided: &str, expected: &str) -> bool {
    provided.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn success() -> Value {
        json!({"Body":{"stkCallback":{
        "MerchantRequestID":"29115-34620561-1","CheckoutRequestID":"ws_CO_191220191020363925",
        "ResultCode":0,"ResultDesc":"The service request is processed successfully.",
        "CallbackMetadata":{"Item":[
            {"Name":"Amount","Value":1.00},
            {"Name":"MpesaReceiptNumber","Value":"NLJ7RT61SV"},
            {"Name":"PhoneNumber","Value":254708374149_i64}
        ]}}}})
    }

    #[test]
    fn parses_success() {
        let cb = parse_stk_callback(&success()).unwrap();
        assert_eq!(cb.result_code, 0);
        assert_eq!(cb.amount_minor, Some(100));
        assert_eq!(cb.receipt.as_deref(), Some("NLJ7RT61SV"));
        assert_eq!(cb.checkout_request_id, "ws_CO_191220191020363925");
    }

    #[test]
    fn parses_failure_without_metadata() {
        let v = json!({"Body":{"stkCallback":{
            "MerchantRequestID":"m","CheckoutRequestID":"c","ResultCode":1032,"ResultDesc":"Request cancelled by user"}}});
        let cb = parse_stk_callback(&v).unwrap();
        assert_eq!(cb.result_code, 1032);
        assert_eq!(cb.amount_minor, None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_stk_callback(&json!({"hello":"world"})).is_err());
    }

    #[test]
    fn token_check() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
    }
}
