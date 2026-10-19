//! CI helper: fail if cargo tree contains banned crates (A13).
//! Run: `cargo run -p hx-native --bin check_deps`

fn main() {
    let output = std::process::Command::new("cargo")
        .args(["tree", "-p", "hx-native", "--prefix", "none"])
        .output()
        .expect("failed to run cargo tree");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        std::process::exit(1);
    }
    let tree = String::from_utf8_lossy(&output.stdout);
    let mut bad = Vec::new();
    for line in tree.lines() {
        let pkg = line.split_whitespace().next().unwrap_or("");
        if pkg == "wgpu"
            || pkg.starts_with("wgpu-")
            || pkg.starts_with("mainnet")
            || pkg == "graph_bridge"
            || pkg == "blitz-net"
        {
            bad.push(pkg.to_string());
        }
    }
    if !bad.is_empty() {
        eprintln!("A13 banned dependencies present: {bad:?}");
        std::process::exit(1);
    }
    println!("A13 ok: no wgpu / mainnet / graph_bridge / blitz-net");
}
