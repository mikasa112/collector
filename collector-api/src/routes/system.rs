use salvo::Router;

use crate::{handlers, middleware::auth::auth_handler};

/// 系统管理相关路由：配置读写、备份/回滚、重启、状态查询
///
/// 整组统一鉴权（包括 GET）：config.json 里含 mqtt 明文密码等敏感信息
pub(crate) fn router() -> Router {
    Router::with_path("system")
        .hoop(auth_handler())
        .push(
            Router::with_path("config")
                .get(handlers::system::get_config)
                .put(handlers::system::put_config),
        )
        .push(
            Router::with_path("backups")
                .get(handlers::system::list_backups)
                .push(Router::with_path("{name}/restore").post(handlers::system::restore_backup)),
        )
        .push(Router::with_path("restart").post(handlers::system::restart))
        .push(Router::with_path("status").get(handlers::system::status))
        .push(
            Router::with_path("scripts")
                .get(handlers::script::list_scripts)
                .push(
                    Router::with_path("file")
                        .get(handlers::script::get_script)
                        .put(handlers::script::put_script)
                        .post(handlers::script::post_script)
                        .delete(handlers::script::delete_script),
                ),
        )
}
