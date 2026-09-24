use rust_embed::RustEmbed;
use salvo::Router;
use salvo::serve_static::static_embed;

/// 编译期嵌入的前端产物（`web/dist`），部署时随后端二进制一起分发。
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../web/dist"]
struct WebAssets;

/// 兜底路由：非 `/v1/*`、`/ws/*` 的请求都交给嵌入的前端资源处理，
/// 命中不到任何文件时回退到 `index.html`。
pub(crate) fn router() -> Router {
    Router::with_path("{**path}").get(
        static_embed::<WebAssets>()
            .defaults(vec!["index.html"])
            .fallback("index.html"),
    )
}
