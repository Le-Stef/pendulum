mod clock;
mod config;
mod gps_nmea;
mod gps_reader;
mod packet;
mod security;
mod server;
mod stats;
mod web_server;

use anyhow::{Context, Result};
use clock::{ClockSource, GpsNmeaClock, SystemClock};
use config::Config;
use gps_reader::GpsReader;
use server::NtpServer;
use stats::StatsManager;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use web_server::WebServer;

fn main() -> Result<()> {
    // Initialiser les logs
    init_logging()?;

    info!("Pendulum NTP Server v{}", env!("CARGO_PKG_VERSION"));
    info!("Professional GPS-synchronized NTP server");

    // Charger la configuration
    let config_path = get_config_path();
    let config = load_or_create_config(&config_path)?;

    // Afficher la configuration
    info!("Configuration:");
    info!("  Bind address: {}", config.server.bind_address);
    info!("  Clock source: {}", config.clock.source);
    info!("  Rate limiting: {}", config.security.enable_rate_limiting);

    // Créer le gestionnaire de statistiques d'abord
    let stats_manager = StatsManager::new();
    let stats_arc = stats_manager.clone_arc();

    // Créer la source d'horloge appropriée
    let clock: Arc<dyn ClockSource> = match config.clock.source.as_str() {
        "system" => {
            info!("Using system clock");
            Arc::new(SystemClock::new())
        }
        "gps" => {
            if let Some(ref gps_config) = config.clock.gps {
                info!("Using GPS clock");
                info!("  Enabled: {}", gps_config.enabled);
                info!("  Serial port: {}", gps_config.serial_port);
                info!("  Baud rate: {}", gps_config.baud_rate);
                info!("  PPS via CTS: {}", gps_config.pps_enabled);
                info!("  Min satellites: {}", gps_config.min_satellites);

                let gps_clock = Arc::new(GpsNmeaClock::new(gps_config.sync_timeout));

                // Démarrer le thread de lecture GPS si activé
                if gps_config.enabled {
                    info!("Starting GPS reader thread...");

                    let reader = GpsReader::new(
                        gps_config.clone(),
                        Arc::clone(&gps_clock),
                        Arc::clone(&stats_arc),
                    );

                    // Démarrer le thread GPS (avec reconnexion automatique)
                    let _gps_thread = reader.start();

                    info!("GPS reader thread started successfully");
                    info!("The server will use GPS time when available, system clock otherwise");

                    // Attendre un peu pour laisser le GPS se connecter
                    // (non bloquant, le serveur démarre quand même)
                    std::thread::sleep(std::time::Duration::from_secs(2));
                } else {
                    warn!("GPS module is disabled in configuration");
                    warn!("Server will use system clock only");
                }

                gps_clock as Arc<dyn ClockSource>
            } else {
                error!("GPS clock source selected but no GPS configuration found");
                std::process::exit(1);
            }
        }
        _ => {
            error!("Unknown clock source: {}", config.clock.source);
            std::process::exit(1);
        }
    };

    // Afficher les infos de l'horloge
    info!("Clock information:");
    info!("  Stratum: {}", clock.stratum());
    info!("  Precision: 2^{} seconds", clock.precision());
    info!(
        "  Reference ID: {}",
        String::from_utf8_lossy(&clock.reference_id())
    );

    // Initialiser les infos d'horloge dans les stats
    stats_manager.update_clock(|clock_info| {
        clock_info.stratum = clock.stratum();
        clock_info.reference_id = String::from_utf8_lossy(&clock.reference_id()).to_string();
        clock_info.precision = clock.precision();
    });

    // Démarrer le serveur web
    let web_bind = format!("{}:{}", config.webserver.bind_address, config.webserver.port);
    info!("Starting web interface on http://{}", web_bind);
    let web_server = WebServer::new(
        web_bind,
        Arc::clone(&stats_arc),
        Arc::clone(&clock),
    );
    let _web_thread = web_server.start();

    // Gérer Ctrl+C avec confirmation à double pression
    let shutdown_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctrl_c_count = Arc::new(std::sync::atomic::AtomicU8::new(0));

    let shutdown_clone = Arc::clone(&shutdown_requested);
    let count_clone = Arc::clone(&ctrl_c_count);

    ctrlc::set_handler(move || {
        let count = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if count == 0 {
            // Première pression
            warn!("Ctrl+C détecté. Appuyez à nouveau dans les 5 secondes pour arrêter le serveur.");

            // Thread qui désamorce après 5 secondes
            let count_disarm = Arc::clone(&count_clone);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let current = count_disarm.load(std::sync::atomic::Ordering::SeqCst);
                if current == 1 {
                    // Pas de deuxième pression, désamorcer
                    count_disarm.store(0, std::sync::atomic::Ordering::SeqCst);
                    info!("Arrêt annulé. Le serveur continue.");
                }
            });
        } else {
            // Deuxième pression (ou plus)
            warn!("Arrêt confirmé. Fermeture du serveur...");
            shutdown_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            // Forcer la sortie si le serveur ne répond pas après 2 secondes
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(2));
                error!("Arrêt forcé (timeout)");
                std::process::exit(0);
            });
        }
    })
    .context("Failed to set Ctrl+C handler")?;

    // Créer et démarrer le serveur NTP avec le flag shutdown
    let server = NtpServer::new(config, clock, Arc::clone(&stats_arc));

    info!("Starting NTP server...");
    info!("Web interface: http://localhost:8080");
    info!("Press Ctrl+C twice (within 5 seconds) to stop");

    // Démarrer le serveur avec le flag shutdown
    match server.run(Arc::clone(&shutdown_requested)) {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("Server error: {:#}", e);
            Err(e)
        }
    }
}

/// Initialise le système de logging
fn init_logging() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .context("Failed to create log filter")?;

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).with_thread_ids(false))
        .with(filter)
        .init();

    Ok(())
}

/// Obtient le chemin du fichier de configuration
fn get_config_path() -> PathBuf {
    // Vérifier les arguments de ligne de commande
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        return PathBuf::from(&args[1]);
    }

    // Sinon, utiliser le chemin par défaut
    #[cfg(target_os = "linux")]
    return PathBuf::from("/etc/pendulum/config.toml");

    #[cfg(target_os = "windows")]
    return PathBuf::from("config.toml");

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    return PathBuf::from("config.toml");
}

/// Charge la configuration ou crée un fichier exemple
fn load_or_create_config(path: &PathBuf) -> Result<Config> {
    if path.exists() {
        info!("Loading configuration from {}", path.display());
        Config::from_file(path)
    } else {
        warn!("Configuration file not found: {}", path.display());
        warn!("Creating example configuration...");

        // Créer le répertoire parent si nécessaire
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .context("Failed to create config directory")?;
            }
        }

        // Créer une config exemple
        Config::create_example_config(path)
            .context("Failed to create example config")?;

        info!("Example configuration created at {}", path.display());
        info!("Please edit the configuration file and restart the server.");

        // Charger la config créée
        Config::from_file(path)
    }
}
