// Sieve of Eratosthenes (Vec<bool> and a while loop, index bounds checks)
fn main() {
    let n = 20_000_000usize;
    let mut is = vec![true; n + 1];
    is[0] = false;
    is[1] = false;
    let mut i = 2;
    while i * i <= n {
        if is[i] {
            let mut j = i * i;
            while j <= n {
                is[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    let mut count = 0u64;
    let mut sum = 0u64;
    for (k, &p) in is.iter().enumerate() {
        if p {
            count += 1;
            sum = sum.wrapping_add(k as u64);
        }
    }
    println!("{count} {sum}");
}
