//! flux-commerce — the **1% sales** mold (Wickes commerce checkout).
//!
//! A shop's catalog, stock and carts, and a checkout that settles every order through
//! [`flux_money_kit`]: the merchant receives the sale, the house keeps a 1% protocol fee,
//! the stock ledger records what left the shelf. Guest carts, split payments, coupons and
//! refunds are in; storefront, POS and fulfilment are the next layers on top (catalog §E–§F).
//!
//! **The one rule of this crate:** everything is checked *before* anything moves. A refused
//! checkout — unknown item, empty shelf, short wallet, wrong split — leaves the treasury,
//! the stock and the cart exactly as they were.
//!
//! `wickes-cms` products map onto [`Product`]; this crate does not depend on it, so the
//! mold stays pure std and copy-pasteable like its siblings.

use flux_money_kit::{skim, MoneyError, Receipt, Treasury};
use std::collections::{BTreeMap, HashMap};

/// House wallet that collects the protocol fee.
pub const HOUSE: &str = "commerce-house";
/// The protocol fee on every sale, in basis points (100 = 1%).
pub const PROTOCOL_FEE_BPS: u32 = 100;

/// One sellable item. `price` is in micro-units (µ) of `token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub sku: String,
    pub title: String,
    pub price: u128,
    pub token: String,
}

/// Percent-off coupon, in basis points (1000 = 10%), optionally capped in µ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coupon {
    pub code: String,
    pub off_bps: u32,
    pub max_off: Option<u128>,
}

/// Why a checkout (or refund) was refused. Nothing moved when one of these is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShopError {
    UnknownSku(String),
    UnknownCart(String),
    UnknownOrder(String),
    EmptyCart,
    OutOfStock { sku: String, have: u64, want: u64 },
    MixedTokens,
    SplitMismatch { total: u128, paid: u128 },
    AlreadyRefunded(String),
    Money(MoneyError),
}

impl From<MoneyError> for ShopError {
    fn from(e: MoneyError) -> Self { ShopError::Money(e) }
}

pub type Result<T> = std::result::Result<T, ShopError>;

/// A cart. `owner = None` is a guest cart: the payer is named only at checkout.
#[derive(Debug, Clone, Default)]
pub struct Cart {
    pub owner: Option<String>,
    pub lines: BTreeMap<String, u64>,
}

/// What an order costs, before any money moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub token: String,
    pub subtotal: u128,
    pub discount: u128,
    pub total: u128,
    /// What the merchant receives: `total - fee`.
    pub merchant_net: u128,
    /// The 1% protocol fee (floored, so rounding favours the merchant).
    pub fee: u128,
}

/// A settled order.
#[derive(Debug, Clone)]
pub struct Order {
    pub id: String,
    pub lines: BTreeMap<String, u64>,
    pub quote: Quote,
    pub receipts: Vec<Receipt>,
    pub refunded: bool,
}

/// One entry of the stock-move ledger: positive = restock/return, negative = sold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMove {
    pub sku: String,
    pub delta: i64,
    pub reason: String,
}

/// A shop: catalog, stock, carts, orders, and the merchant wallet that gets paid.
#[derive(Debug, Clone)]
pub struct Shop {
    pub merchant: String,
    products: HashMap<String, Product>,
    stock: HashMap<String, u64>,
    pub moves: Vec<StockMove>,
    carts: HashMap<String, Cart>,
    orders: HashMap<String, Order>,
    next_order: u64,
}

impl Shop {
    pub fn new(merchant: &str) -> Self {
        Shop {
            merchant: merchant.to_string(), products: HashMap::new(), stock: HashMap::new(),
            moves: Vec::new(), carts: HashMap::new(), orders: HashMap::new(), next_order: 1,
        }
    }

    pub fn list_product(&mut self, p: Product) { self.products.insert(p.sku.clone(), p); }

    pub fn restock(&mut self, sku: &str, qty: u64) -> Result<()> {
        if !self.products.contains_key(sku) { return Err(ShopError::UnknownSku(sku.into())); }
        *self.stock.entry(sku.into()).or_insert(0) += qty;
        self.moves.push(StockMove { sku: sku.into(), delta: qty as i64, reason: "restock".into() });
        Ok(())
    }

    pub fn stock(&self, sku: &str) -> u64 { *self.stock.get(sku).unwrap_or(&0) }

    /// Open a cart (`owner = None` for a guest).
    pub fn open_cart(&mut self, cart_id: &str, owner: Option<&str>) {
        self.carts.insert(cart_id.into(), Cart { owner: owner.map(String::from), lines: BTreeMap::new() });
    }

    pub fn add_to_cart(&mut self, cart_id: &str, sku: &str, qty: u64) -> Result<()> {
        if !self.products.contains_key(sku) { return Err(ShopError::UnknownSku(sku.into())); }
        let cart = self.carts.get_mut(cart_id).ok_or_else(|| ShopError::UnknownCart(cart_id.into()))?;
        *cart.lines.entry(sku.into()).or_insert(0) += qty;
        Ok(())
    }

    pub fn cart(&self, cart_id: &str) -> Option<&Cart> { self.carts.get(cart_id) }

    /// Price a cart — stock, token and coupon checked — without touching anything.
    pub fn quote(&self, cart_id: &str, coupon: Option<&Coupon>) -> Result<Quote> {
        let cart = self.carts.get(cart_id).ok_or_else(|| ShopError::UnknownCart(cart_id.into()))?;
        if cart.lines.is_empty() { return Err(ShopError::EmptyCart); }
        let mut token: Option<&str> = None;
        let mut subtotal: u128 = 0;
        for (sku, &qty) in &cart.lines {
            let p = self.products.get(sku).ok_or_else(|| ShopError::UnknownSku(sku.clone()))?;
            match token {
                None => token = Some(&p.token),
                Some(t) if t != p.token => return Err(ShopError::MixedTokens),
                _ => {}
            }
            let have = self.stock(sku);
            if have < qty { return Err(ShopError::OutOfStock { sku: sku.clone(), have, want: qty }); }
            let line = p.price.checked_mul(qty as u128).ok_or(MoneyError::Overflow)?;
            subtotal = subtotal.checked_add(line).ok_or(MoneyError::Overflow)?;
        }
        let discount = match coupon {
            None => 0,
            Some(c) => {
                let (_, off) = skim(subtotal, c.off_bps)?; // skim's fee half = the discount
                c.max_off.map_or(off, |m| off.min(m))
            }
        };
        let total = subtotal - discount;
        let (merchant_net, fee) = skim(total, PROTOCOL_FEE_BPS)?;
        Ok(Quote { token: token.unwrap_or_default().to_string(), subtotal, discount, total, merchant_net, fee })
    }

    /// Check out with one payer (the cart's owner, or the named payer for a guest cart).
    pub fn checkout(&mut self, t: &mut Treasury, cart_id: &str, payer: &str, coupon: Option<&Coupon>) -> Result<Order> {
        let total = self.quote(cart_id, coupon)?.total;
        self.checkout_split(t, cart_id, &[(payer, total)], coupon)
    }

    /// Check out with several payers; their shares must add up to the total exactly.
    /// Each payer's share is split 99/1 between merchant and house, so the house fee is
    /// paid pro rata and the merchant still receives exactly `total - fee`.
    pub fn checkout_split(&mut self, t: &mut Treasury, cart_id: &str, payers: &[(&str, u128)], coupon: Option<&Coupon>) -> Result<Order> {
        let quote = self.quote(cart_id, coupon)?;
        let paid = payers.iter().try_fold(0u128, |a, (_, x)| a.checked_add(*x)).ok_or(MoneyError::Overflow)?;
        if paid != quote.total { return Err(ShopError::SplitMismatch { total: quote.total, paid }); }
        // Every check BEFORE the first move: a short payer refuses the whole order.
        let mut need: BTreeMap<&str, u128> = BTreeMap::new();
        for (w, x) in payers { *need.entry(w).or_insert(0) += *x; }
        for (w, x) in &need {
            let have = t.balance(w, &quote.token);
            if have < *x {
                return Err(MoneyError::Insufficient { wallet: w.to_string(), token: quote.token.clone(), have, need: *x }.into());
            }
        }
        // Fee shares: floor per payer, the last payer carries the rounding remainder.
        let mut receipts = Vec::new();
        let mut fee_left = quote.fee;
        for (i, (w, share)) in payers.iter().enumerate() {
            let fee_part = if i + 1 == payers.len() { fee_left } else { quote.fee * share / quote.total };
            fee_left -= fee_part;
            if share - fee_part > 0 { receipts.push(t.settle(w, &self.merchant, &quote.token, share - fee_part)?); }
            if fee_part > 0 { receipts.push(t.settle(w, HOUSE, &quote.token, fee_part)?); }
        }
        let cart = self.carts.remove(cart_id).expect("quoted above");
        for (sku, &qty) in &cart.lines {
            *self.stock.get_mut(sku).expect("quoted above") -= qty;
            self.moves.push(StockMove { sku: sku.clone(), delta: -(qty as i64), reason: format!("order {}", self.next_order) });
        }
        let id = format!("ord-{}", self.next_order);
        self.next_order += 1;
        let order = Order { id: id.clone(), lines: cart.lines, quote, receipts, refunded: false };
        self.orders.insert(id, order.clone());
        Ok(order)
    }

    pub fn order(&self, id: &str) -> Option<&Order> { self.orders.get(id) }

    /// Refund an order to `to`: the merchant returns its net and the stock goes back on the
    /// shelf. The 1% protocol fee is not refunded — it paid for a settlement that happened.
    pub fn refund(&mut self, t: &mut Treasury, order_id: &str, to: &str) -> Result<Receipt> {
        let o = self.orders.get(order_id).ok_or_else(|| ShopError::UnknownOrder(order_id.into()))?;
        if o.refunded { return Err(ShopError::AlreadyRefunded(order_id.into())); }
        let r = t.settle(&self.merchant, to, &o.quote.token, o.quote.merchant_net)?;
        let lines = o.lines.clone();
        for (sku, qty) in lines {
            *self.stock.entry(sku.clone()).or_insert(0) += qty;
            self.moves.push(StockMove { sku, delta: qty as i64, reason: format!("refund {order_id}") });
        }
        self.orders.get_mut(order_id).expect("checked above").refunded = true;
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux_money_kit::{MICRO, USDS};

    fn shop() -> (Shop, Treasury) {
        let mut s = Shop::new("merchant");
        s.list_product(Product { sku: "mug".into(), title: "SIGIL mug".into(), price: 12 * MICRO, token: USDS.into() });
        s.list_product(Product { sku: "tee".into(), title: "Glyph tee".into(), price: 25 * MICRO, token: USDS.into() });
        s.restock("mug", 10).unwrap();
        s.restock("tee", 2).unwrap();
        let mut t = Treasury::new();
        t.credit("alice", USDS, 100 * MICRO).unwrap();
        t.credit("bob", USDS, 100 * MICRO).unwrap();
        (s, t)
    }

    #[test]
    fn checkout_pays_merchant_99_house_1_and_conserves() {
        let (mut s, mut t) = shop();
        let before = t.supply(USDS);
        s.open_cart("c1", Some("alice"));
        s.add_to_cart("c1", "mug", 2).unwrap();
        s.add_to_cart("c1", "tee", 1).unwrap();
        let o = s.checkout(&mut t, "c1", "alice", None).unwrap();
        assert_eq!(o.quote.total, 49 * MICRO);
        assert_eq!(o.quote.fee, 49 * MICRO / 100);
        assert_eq!(o.quote.merchant_net + o.quote.fee, o.quote.total);
        assert_eq!(t.balance("merchant", USDS), o.quote.merchant_net);
        assert_eq!(t.balance(HOUSE, USDS), o.quote.fee);
        assert_eq!(t.balance("alice", USDS), 51 * MICRO);
        assert_eq!(t.supply(USDS), before);
        assert_eq!((s.stock("mug"), s.stock("tee")), (8, 1));
        assert!(s.cart("c1").is_none());
    }

    #[test]
    fn guest_cart_is_paid_by_whoever_checks_out() {
        let (mut s, mut t) = shop();
        s.open_cart("g", None);
        s.add_to_cart("g", "mug", 1).unwrap();
        s.checkout(&mut t, "g", "bob", None).unwrap();
        assert_eq!(t.balance("bob", USDS), 88 * MICRO);
    }

    #[test]
    fn a_refused_checkout_moves_nothing() {
        let (mut s, mut t) = shop();
        s.open_cart("c", Some("alice"));
        s.add_to_cart("c", "tee", 3).unwrap(); // only 2 on the shelf
        assert_eq!(s.checkout(&mut t, "c", "alice", None).unwrap_err(), ShopError::OutOfStock { sku: "tee".into(), have: 2, want: 3 });
        s.open_cart("d", None);
        s.add_to_cart("d", "mug", 9).unwrap(); // 108 > alice's 100
        assert!(matches!(s.checkout(&mut t, "d", "alice", None), Err(ShopError::Money(MoneyError::Insufficient { .. }))));
        assert_eq!(t.balance("alice", USDS), 100 * MICRO);
        assert_eq!(t.balance("merchant", USDS), 0);
        assert_eq!((s.stock("mug"), s.stock("tee")), (10, 2));
        assert!(s.cart("c").is_some() && s.cart("d").is_some());
    }

    #[test]
    fn split_payment_must_add_up_and_still_conserves() {
        let (mut s, mut t) = shop();
        let before = t.supply(USDS);
        s.open_cart("c", None);
        s.add_to_cart("c", "tee", 2).unwrap(); // 50
        assert_eq!(
            s.checkout_split(&mut t, "c", &[("alice", 20 * MICRO), ("bob", 20 * MICRO)], None).unwrap_err(),
            ShopError::SplitMismatch { total: 50 * MICRO, paid: 40 * MICRO }
        );
        let o = s.checkout_split(&mut t, "c", &[("alice", 30 * MICRO), ("bob", 20 * MICRO)], None).unwrap();
        assert_eq!(t.balance("alice", USDS), 70 * MICRO);
        assert_eq!(t.balance("bob", USDS), 80 * MICRO);
        assert_eq!(t.balance("merchant", USDS), o.quote.merchant_net);
        assert_eq!(t.balance(HOUSE, USDS), o.quote.fee);
        assert_eq!(t.supply(USDS), before);
    }

    #[test]
    fn coupon_takes_percent_off_with_a_cap() {
        let (mut s, _t) = shop();
        s.open_cart("c", None);
        s.add_to_cart("c", "mug", 5).unwrap(); // 60
        let ten = Coupon { code: "TEN".into(), off_bps: 1000, max_off: None };
        assert_eq!(s.quote("c", Some(&ten)).unwrap().discount, 6 * MICRO);
        let capped = Coupon { code: "CAP".into(), off_bps: 5000, max_off: Some(5 * MICRO) };
        let q = s.quote("c", Some(&capped)).unwrap();
        assert_eq!((q.discount, q.total), (5 * MICRO, 55 * MICRO));
    }

    #[test]
    fn mixed_token_cart_is_refused() {
        let (mut s, _t) = shop();
        s.list_product(Product { sku: "nft".into(), title: "Glyph".into(), price: MICRO, token: "QUG".into() });
        s.restock("nft", 1).unwrap();
        s.open_cart("c", None);
        s.add_to_cart("c", "mug", 1).unwrap();
        s.add_to_cart("c", "nft", 1).unwrap();
        assert_eq!(s.quote("c", None).unwrap_err(), ShopError::MixedTokens);
    }

    #[test]
    fn refund_returns_net_restocks_once_and_keeps_the_fee() {
        let (mut s, mut t) = shop();
        s.open_cart("c", Some("alice"));
        s.add_to_cart("c", "mug", 1).unwrap();
        let o = s.checkout(&mut t, "c", "alice", None).unwrap();
        s.refund(&mut t, &o.id, "alice").unwrap();
        assert_eq!(t.balance("alice", USDS), 100 * MICRO - o.quote.fee);
        assert_eq!(t.balance("merchant", USDS), 0);
        assert_eq!(s.stock("mug"), 10);
        assert_eq!(s.refund(&mut t, &o.id, "alice").unwrap_err(), ShopError::AlreadyRefunded(o.id.clone()));
        assert_eq!(s.moves.last().unwrap().reason, format!("refund {}", o.id));
    }
}
