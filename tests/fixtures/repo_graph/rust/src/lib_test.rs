use repo_graph_rust_fixture::greet;

#[test]
fn it_greets() {
    assert_eq!(greet("anvil"), "hello, anvil");
}
