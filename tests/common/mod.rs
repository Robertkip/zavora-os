use std::path::PathBuf;

pub fn load_env() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let _ = dotenvy::from_filename(manifest.join(".env.local"));
    let _ = dotenvy::from_filename(manifest.join(".env"));
    dotenvy::dotenv().ok();
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub struct McpPaths {
    pub worksheet: PathBuf,
    pub docx: PathBuf,
    pub slides: PathBuf,
    pub news: PathBuf,
    pub weather: PathBuf,
}

pub fn mcp_paths() -> McpPaths {
    let m = manifest_dir();
    McpPaths {
        worksheet: m.join("../mcp-servers/worksheet-mcp/target/release/excel-mcp-server"),
        docx: m.join("../mcp-servers/docx-mcp/target/release/docx-mcp-server"),
        slides: m.join("../mcp-servers/mcp-slides/target/release/slides-mcp-server"),
        news: m.join("../mcp-servers/mcp-news/target/release/mcp-news"),
        weather: m.join("../mcp-servers/mcp-weather/target/release/mcp-weather"),
    }
}

pub fn assert_mcp_binaries_exist(paths: &McpPaths) {
    for (name, path) in [
        ("worksheet", &paths.worksheet),
        ("docx", &paths.docx),
        ("slides", &paths.slides),
        ("news", &paths.news),
        ("weather", &paths.weather),
    ] {
        assert!(
            path.exists(),
            "MCP binary missing for {name}: {}\n\
             Build with: (cd mcp-servers/{name}-mcp && cargo build --release)",
            path.display(),
        );
    }
}

pub fn google_api_key() -> String {
    load_env();
    match std::env::var("GOOGLE_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => panic!("GOOGLE_API_KEY must be set in .env for validation tests"),
    }
}

pub fn gemini_model() -> String {
    load_env();
    std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.1-flash-lite".into())
}

/// `DATABASE_URL` from `.env` — required for M9 postgres validation tests.
pub fn database_url() -> String {
    load_env();
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "DATABASE_URL must be set — start Postgres with: docker compose up -d"
        )
    })
}

pub async fn postgres_pool() -> sqlx::PgPool {
    spatial_os::db::connect(&database_url())
        .await
        .expect("postgres connect failed — is spatial-os-postgres running on 5434?")
}