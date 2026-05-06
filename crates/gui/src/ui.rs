pub fn status_text(n: u32) -> String {
    if n == 0 {
        "Not installed".to_string()
    } else {
        format!("{} instance(s) to keep", n)
    }
}
