fn main() {
    let schema = capsule::config::json_schema();
    let out = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema/config.schema.json");
    std::fs::write(path, out).expect("failed to write schema file");
    eprintln!("wrote {path}");
}
