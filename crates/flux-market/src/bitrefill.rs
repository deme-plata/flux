//! bitrefill.rs — the SPEND leg. Buy gift cards / top-ups / food-delivery
//! vouchers with Lightning via Bitrefill. This is the "rocky, order me a pizza"
//! path: search a delivery gift card → create an LN invoice → `ln_pay` it →
//! redeem the code in the app.
//!
//! SAFETY: this NEVER auto-spends. A purchase needs `BITREFILL_API_ID` +
//! `BITREFILL_API_SECRET` (your account, HTTP Basic) AND the caller explicitly
//! pays the returned Lightning invoice. The client only builds requests + parses
//! responses. Endpoint/field shapes follow Bitrefill's v2 API; verify against the
//! live API with a real key before trusting field names (untested w/o creds).

use serde::Deserialize;
use std::time::Duration;

/// Bitrefill API credentials (HTTP Basic: api_id:api_secret).
pub struct BitrefillCreds {
    pub api_id: String,
    pub api_secret: String,
}

impl BitrefillCreds {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            api_id: std::env::var("BITREFILL_API_ID").map_err(|_| "set BITREFILL_API_ID".to_string())?,
            api_secret: std::env::var("BITREFILL_API_SECRET").map_err(|_| "set BITREFILL_API_SECRET".to_string())?,
        })
    }
    fn basic(&self) -> String {
        use std::fmt::Write;
        // base64(api_id:api_secret) — minimal inline encoder (no extra dep).
        let raw = format!("{}:{}", self.api_id, self.api_secret);
        let mut out = String::new();
        let b = raw.as_bytes();
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in b.chunks(3) {
            let (b0, b1, b2) = (chunk[0] as u32, *chunk.get(1).unwrap_or(&0) as u32, *chunk.get(2).unwrap_or(&0) as u32);
            let n = (b0 << 16) | (b1 << 8) | b2;
            let _ = write!(out, "{}{}", T[(n >> 18) as usize & 63] as char, T[(n >> 12) as usize & 63] as char);
            let _ = write!(out, "{}", if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            let _ = write!(out, "{}", if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        format!("Basic {out}")
    }
}

/// A purchasable Bitrefill product (gift card / top-up / voucher).
#[derive(Debug, Clone, Deserialize)]
pub struct Product {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub country_code: String,
    #[serde(default)]
    pub currency: String,
}

#[derive(Deserialize)]
struct ProductList {
    #[serde(default)]
    data: Vec<Product>,
}

/// An order's Lightning payment details (what `ln_pay` settles).
#[derive(Debug, Clone, Deserialize)]
pub struct LnOrder {
    pub id: String,
    /// BOLT11 invoice to pay.
    #[serde(default)]
    pub lightning_invoice: String,
    #[serde(default)]
    pub satoshi_price: u64,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("flux-market/0.1 bitrefill")
        .build()
        .map_err(|e| e.to_string())
}

/// Search the catalog (e.g. "pizza", "uber eats", "wolt"). Read-only.
pub fn search_products(creds: &BitrefillCreds, query: &str) -> Result<Vec<Product>, String> {
    let url = format!("https://api.bitrefill.com/v2/products?search={}", urlenc(query));
    let list: ProductList = client()?
        .get(&url)
        .header("Authorization", creds.basic())
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    Ok(list.data)
}

/// Create a Lightning-paid order for `product_id` at `value` (product currency).
/// Returns the BOLT11 to pay. DOES NOT pay it — caller uses `ln_pay`.
pub fn create_lightning_order(creds: &BitrefillCreds, product_id: &str, value: f64) -> Result<LnOrder, String> {
    let body = serde_json::json!({
        "product_id": product_id,
        "value": value,
        "payment_method": "lightning"
    });
    let order: LnOrder = client()?
        .post("https://api.bitrefill.com/v2/invoices")
        .header("Authorization", creds.basic())
        .json(&body)
        .send()
        .map_err(|e| format!("connect: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http: {e}"))?
        .json()
        .map_err(|e| format!("decode: {e}"))?;
    Ok(order)
}

/// A curated real-world spend option; `query` resolves to the live Bitrefill
/// product via [`search_products`] at order time.
#[derive(Debug, Clone)]
pub struct FoodOption {
    pub name: &'static str,
    pub query: &'static str,
    pub emoji: &'static str,
}

/// Denmark (DK) food gift cards on Bitrefill — Viktor's region, confirmed 2026.
/// "rocky, order me a pizza" picks one of these; `query` finds the real product.
pub fn dk_food_menu() -> Vec<FoodOption> {
    vec![
        FoodOption { name: "ILD.PIZZA DK", query: "ild pizza", emoji: "🍕" },
        FoodOption { name: "Sunset Boulevard", query: "sunset boulevard", emoji: "🥪" },
        FoodOption { name: "McDonald's DK", query: "mcdonalds denmark", emoji: "🍔" },
        FoodOption { name: "Restaurant Flammen", query: "flammen", emoji: "🍖" },
        FoodOption { name: "Early Bird", query: "early bird", emoji: "🍳" },
    ]
}

/// Convenience: search Bitrefill for a curated option's live product list.
pub fn find_food(creds: &BitrefillCreds, option: &FoodOption) -> Result<Vec<Product>, String> {
    search_products(creds, option.query)
}

fn urlenc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push_str("%20"),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_encodes() {
        let c = BitrefillCreds { api_id: "id".into(), api_secret: "secret".into() };
        // base64("id:secret") = "aWQ6c2VjcmV0"
        assert_eq!(c.basic(), "Basic aWQ6c2VjcmV0");
    }

    #[test]
    fn order_json_parses() {
        let j = r#"{"id":"ord_1","lightning_invoice":"lnbc100n1...","satoshi_price":21000}"#;
        let o: LnOrder = serde_json::from_str(j).unwrap();
        assert_eq!(o.satoshi_price, 21000);
        assert!(o.lightning_invoice.starts_with("lnbc"));
    }

    #[test]
    fn from_env_errors_without_creds() {
        std::env::remove_var("BITREFILL_API_ID");
        assert!(BitrefillCreds::from_env().is_err());
    }

    #[test]
    fn dk_food_menu_has_the_options() {
        let m = dk_food_menu();
        assert_eq!(m.len(), 5);
        assert!(m.iter().any(|f| f.name == "ILD.PIZZA DK"));
        assert!(m.iter().any(|f| f.name == "Restaurant Flammen"));
    }
}
