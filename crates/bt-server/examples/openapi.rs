#![forbid(unsafe_code)]

#[path = "../src/openapi.rs"]
mod openapi;

fn main() {
    print!("{}", openapi::openapi_json_pretty());
}
