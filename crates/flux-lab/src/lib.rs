pub fn dca_simulate(prices: &[u64], buy_per_period: u64) -> (u128, u128) {
    let mut total_units_micro = 0u128;
    for price in prices {
        let units_bought = (buy_per_period as u128) * 1_000_000 / (*price as u128);
        total_units_micro += units_bought;
    }
    let total_spent = buy_per_period as u128 * prices.len() as u128;
    if total_units_micro == 0 {
        return (0, 0);
    }
    let avg_cost_micro = (total_spent * 1_000_000) / total_units_micro;
    (total_units_micro, avg_cost_micro)
}

#[test]
fn test_flat_price() {
    let prices = [100];
    let buy_per_period = 100;
    let (units, avg) = dca_simulate(&prices, buy_per_period);
    assert_eq!(units, 1_000_000);
    assert_eq!(avg, 100);
}

#[test]
fn test_varying_prices() {
    let prices = [200, 400];
    let buy_per_period = 100;
    let (units, avg) = dca_simulate(&prices, buy_per_period);
    assert_eq!(units, 750_000);
    assert_eq!(avg, 266);
}

#[test]
fn test_empty_prices() {
    let prices: [u64; 0] = [];
    let buy_per_period = 100;
    let (units, avg) = dca_simulate(&prices, buy_per_period);
    assert_eq!(units, 0);
    assert_eq!(avg, 0);
}

#[test]
fn test_zero_buy() {
    let prices = [100];
    let buy_per_period = 0;
    let (units, avg) = dca_simulate(&prices, buy_per_period);
    assert_eq!(units, 0);
    assert_eq!(avg, 0);
}