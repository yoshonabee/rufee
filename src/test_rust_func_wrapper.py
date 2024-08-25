import rufee
import peewee

def test_rust_func_wrapper():
    class A(rufee.Node):
        def __init__(self):
            self.counter = 0
    

    a = A()
    b = a.coerce(False)
    print(a, b)
    print(id(a), id(b))


def test():
    test_rust_func_wrapper()
    print("Ok")

if __name__ == "__main__":
    test()
