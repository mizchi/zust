mod source {
    pub struct Tool;
}
mod m {
    #[allow(unused_imports)]
    use crate::source::Tool as r#Local;
    struct Local {}
}
fn main() {}
