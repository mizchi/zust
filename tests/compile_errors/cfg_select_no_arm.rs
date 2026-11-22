fn main() {
    let _x = 1;
    cfg_select! { false => { return; } }
}
