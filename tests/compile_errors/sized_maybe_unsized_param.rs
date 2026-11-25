fn sized<X>(_: &X) {}

fn relaxed<X: ?Sized>(x: &X) {
    sized::<X>(x);
}

fn main() {
    relaxed("abc");
}
