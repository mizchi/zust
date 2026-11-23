#[derive(PartialEq, Eq)]
enum Value {
    Boolean(Option<bool>),
    Float(Option<f64>),
}
fn main() {
    let v = Value::Boolean(None);
    let _ = v == Value::Float(Some(1.0));
}
