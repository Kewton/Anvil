from .util import upper

def greet(name: str) -> str:
    return upper(f"hello, {name}")

class Greeter:
    def __init__(self, name: str) -> None:
        self.name = name
