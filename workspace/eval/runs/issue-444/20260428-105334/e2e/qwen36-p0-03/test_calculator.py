from calculator import apply_discount


def test_apply_discount():
    assert apply_discount(100, 0.2) == 80

