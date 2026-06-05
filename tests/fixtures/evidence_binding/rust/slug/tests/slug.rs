// Issue #989 fixture: imports the crate-under-test as `slug`, but the package
// is `slugify`. Inert fixture data (subdirectory of tests/fixtures, never
// compiled as an integration test).
use slug::slugify;

#[test]
fn slugifies_ascii() {
    assert_eq!(slugify("Hello World"), "hello-world");
}
