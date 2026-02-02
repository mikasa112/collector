use collector_cmd::{cmd, init_tracing};
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main]
async fn main() {
    let _log = init_tracing();
    let _ = cmd().await;
}
