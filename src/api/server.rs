use crate::api::routes::{create_router, AppState};
use crate::core::pool::StoragePool;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use log::info;



pub struct ApiServer;

impl ApiServer {
    pub async fn start(pool: Arc<StoragePool>) -> std::io::Result<()> {
        let (host, port) = {
            let cfg = pool.config.read().await;
            (cfg.api.host.clone(), cfg.api.port)
        };

        let state = AppState { pool };
        let app = create_router(state);

        let addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8989)));

        info!("Starting Web UI Dashboard and API server on http://{}", addr);

        let listener = TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
