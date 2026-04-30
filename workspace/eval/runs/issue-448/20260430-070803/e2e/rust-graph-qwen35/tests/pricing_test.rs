use issue448_rust_graph::pricing::discount_price;

#[test]
fn applies_ten_percent_discount() {
    assert_eq!(discount_price(1000), 900);
}

