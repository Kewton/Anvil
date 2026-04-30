pub fn multiply(left: i32, right: i32) -> i32 {
    left * right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiply_returns_product() {
        assert_eq!(multiply(4, 5), 20);
    }
}
