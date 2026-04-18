// bridge.rs — Rust implementation of extern "rust" fns declared in main.mvl.
//
// Uses ureq (already a project dependency) to perform HTTP GET.
// The return type is String — the MVL side labels it Tainted<String>,
// documenting the IFC convention that all HTTP responses are tainted.

#[no_mangle]
pub extern "Rust" fn http_get(url: String) -> Result<String, String> {
    ureq::get(&url)
        .call()
        .map_err(|e| e.to_string())
        .and_then(|resp| resp.into_string().map_err(|e| e.to_string()))
}
