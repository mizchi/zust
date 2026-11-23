// An inductive cycle proves nothing: `i32: Combo` needs `i32: Dee`, which needs `i32: Dum`, which needs `i32: Dee`
trait Dee {}
trait Dum {}
impl<T: Dum> Dee for T {}
impl<T: Dee> Dum for T {}
trait Combo {}
impl<T: Dee + Dum> Combo for T {}
fn is_combo<T: Combo>(_: T) {}
fn main() {
    is_combo(4);
}
