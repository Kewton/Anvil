from .main import greet


def test_greet_uppercases():
    assert greet("anvil") == "HELLO, ANVIL"
