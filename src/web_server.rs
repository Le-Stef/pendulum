/*!
Module serveur web pour l'interface de monitoring

Fournit :
- Dashboard HTML avec horloge temps-réel
- API REST pour les statistiques
- WebSocket pour mises à jour temps-réel
- Indicateurs GPS/PPS/USB RX/TX
*/

use crate::clock::ClockSource;
use crate::stats::ServerStats;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Html,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info};

/// État partagé du serveur web
#[derive(Clone)]
pub struct WebServerState {
    stats: Arc<std::sync::RwLock<ServerStats>>,
    clock: Arc<dyn ClockSource>,
}

/// Informations temps-réel pour WebSocket
#[derive(Debug, Clone, Serialize)]
struct RealtimeData {
    /// Timestamp NTP complet (64 bits)
    timestamp: u64,

    /// Secondes depuis epoch NTP (1900-01-01)
    seconds: u32,

    /// Fraction (0 à 2^32-1)
    fraction: u32,

    /// Nanosecondes (pour affichage)
    nanos: u32,

    /// Statistiques complètes
    stats: ServerStats,

    /// Timestamp Unix (pour JavaScript Date)
    unix_timestamp_ms: u64,
}

pub struct WebServer {
    bind_addr: String,
    stats: Arc<std::sync::RwLock<ServerStats>>,
    clock: Arc<dyn ClockSource>,
}

impl WebServer {
    pub fn new(
        bind_addr: String,
        stats: Arc<std::sync::RwLock<ServerStats>>,
        clock: Arc<dyn ClockSource>,
    ) -> Self {
        WebServer {
            bind_addr,
            stats,
            clock,
        }
    }

    /// Démarre le serveur web dans un thread Tokio séparé
    pub fn start(self) -> std::thread::JoinHandle<()> {
        info!("Starting web server on {}", self.bind_addr);

        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                if let Err(e) = self.run().await {
                    error!("Web server error: {:#}", e);
                }
            });
        })
    }

    async fn run(self) -> anyhow::Result<()> {
        let state = WebServerState {
            stats: self.stats,
            clock: self.clock,
        };

        // Routes
        let app = Router::new()
            .route("/", get(index_handler))
            .route("/api/stats", get(stats_handler))
            .route("/api/time", get(time_handler))
            .route("/ws", get(websocket_handler))
            .with_state(state);

        // Bind et écoute
        let listener = tokio::net::TcpListener::bind(&self.bind_addr).await?;
        info!("Web server listening on {}", self.bind_addr);

        axum::serve(listener, app).await?;

        Ok(())
    }
}

/// Page d'accueil avec dashboard
async fn index_handler() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}

/// API REST : Statistiques complètes
async fn stats_handler(State(state): State<WebServerState>) -> Json<ServerStats> {
    let stats = state.stats.read().unwrap().clone();
    Json(stats)
}

/// API REST : Temps actuel
async fn time_handler(State(state): State<WebServerState>) -> Json<RealtimeData> {
    let timestamp = state.clock.now();
    let stats = state.stats.read().unwrap().clone();

    let seconds = timestamp.seconds();
    let fraction = timestamp.fraction();

    // Convertir fraction en nanosecondes
    let nanos = ((fraction as u64 * 1_000_000_000) >> 32) as u32;

    // Convertir en timestamp Unix pour JavaScript
    const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
    let unix_timestamp_ms = ((seconds as u64 - NTP_UNIX_OFFSET) * 1000)
        + (nanos as u64 / 1_000_000);

    Json(RealtimeData {
        timestamp: timestamp.0,
        seconds,
        fraction,
        nanos,
        stats,
        unix_timestamp_ms,
    })
}

/// WebSocket pour mises à jour temps-réel
#[axum::debug_handler]
async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<WebServerState>,
) -> axum::response::Response {
    ws.on_upgrade(|socket| websocket_task(socket, state))
}

/// Tâche WebSocket : envoie les mises à jour toutes les 50ms
async fn websocket_task(mut socket: WebSocket, state: WebServerState) {
    loop {
        let timestamp = state.clock.now();
        let stats = state.stats.read().unwrap().clone();

        let seconds = timestamp.seconds();
        let fraction = timestamp.fraction();
        let nanos = ((fraction as u64 * 1_000_000_000) >> 32) as u32;

        const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
        let unix_timestamp_ms = ((seconds as u64 - NTP_UNIX_OFFSET) * 1000)
            + (nanos as u64 / 1_000_000);

        let data = RealtimeData {
            timestamp: timestamp.0,
            seconds,
            fraction,
            nanos,
            stats,
            unix_timestamp_ms,
        };

        let json = match serde_json::to_string(&data) {
            Ok(j) => j,
            Err(_) => break,
        };

        if socket.send(Message::Text(json)).await.is_err() {
            break;
        }

        // Mise à jour toutes les 50ms (20 FPS)
        sleep(Duration::from_millis(50)).await;
    }
}
