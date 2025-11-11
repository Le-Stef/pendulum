use crate::clock::ClockSource;
use crate::config::Config;
use crate::packet::{LeapIndicator, NtpMode, NtpPacket, NtpTimestamp};
use crate::security::{IpFilter, PacketValidator, RateLimiter};
use crate::stats::ServerStats as SharedServerStats;
use anyhow::{Context, Result};
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

/// Statistiques du serveur
pub struct ServerStats {
    pub requests_received: std::sync::atomic::AtomicU64,
    pub requests_processed: std::sync::atomic::AtomicU64,
    pub requests_rejected: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
}

impl ServerStats {
    pub fn new() -> Self {
        ServerStats {
            requests_received: std::sync::atomic::AtomicU64::new(0),
            requests_processed: std::sync::atomic::AtomicU64::new(0),
            requests_rejected: std::sync::atomic::AtomicU64::new(0),
            errors: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn log_stats(&self) {
        let received = self.requests_received.load(std::sync::atomic::Ordering::Relaxed);
        let processed = self.requests_processed.load(std::sync::atomic::Ordering::Relaxed);
        let rejected = self.requests_rejected.load(std::sync::atomic::Ordering::Relaxed);
        let errors = self.errors.load(std::sync::atomic::Ordering::Relaxed);

        info!(
            "Stats: received={}, processed={}, rejected={}, errors={}",
            received, processed, rejected, errors
        );
    }
}

impl Default for ServerStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Serveur NTP
pub struct NtpServer<C: ClockSource + ?Sized> {
    config: Config,
    clock: Arc<C>,
    rate_limiter: Option<RateLimiter>,
    ip_filter: IpFilter,
    stats: Arc<ServerStats>,
    shared_stats: Arc<std::sync::RwLock<SharedServerStats>>,
}

impl<C: ClockSource + ?Sized> NtpServer<C> {
    pub fn new(
        config: Config,
        clock: Arc<C>,
        shared_stats: Arc<std::sync::RwLock<SharedServerStats>>,
    ) -> Self {
        let rate_limiter = if config.security.enable_rate_limiting {
            Some(RateLimiter::new(config.security.max_requests_per_second))
        } else {
            None
        };

        let ip_filter = IpFilter::new(
            config.security.ip_whitelist.clone(),
            config.security.ip_blacklist.clone(),
        );

        NtpServer {
            config,
            clock,
            rate_limiter,
            ip_filter,
            stats: Arc::new(ServerStats::new()),
            shared_stats,
        }
    }

    /// Démarre le serveur NTP
    pub fn run(&self, shutdown: Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
        let socket = UdpSocket::bind(&self.config.server.bind_address)
            .context("Failed to bind UDP socket")?;

        // Configurer un timeout pour recv_from afin de pouvoir vérifier le shutdown flag
        socket.set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .context("Failed to set socket read timeout")?;

        info!("NTP server listening on {}", self.config.server.bind_address);
        info!("Clock source: {}", self.config.clock.source);
        info!("Stratum: {}", self.clock.stratum());

        // Thread pour logger les stats périodiquement et mettre à jour les stats partagées
        let stats_clone = Arc::clone(&self.stats);
        let shared_stats_clone = Arc::clone(&self.shared_stats);
        std::thread::spawn(move || {
            let mut last_requests = 0u64;
            let mut last_tx = Instant::now();

            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));

                // Calculer requests per second
                let current_requests = stats_clone.requests_processed.load(std::sync::atomic::Ordering::Relaxed);
                let requests_per_second = (current_requests - last_requests) as u32;
                last_requests = current_requests;

                // Mettre à jour les stats partagées
                if let Ok(mut stats) = shared_stats_clone.write() {
                    stats.ntp.requests_per_second = requests_per_second;

                    // Mettre à jour last_tx_ms
                    let tx_elapsed_ms = last_tx.elapsed().as_millis() as u64;
                    if stats.ntp.last_tx_ms == 0 {
                        // Un TX vient de se produire, réinitialiser le timer
                        last_tx = Instant::now();
                    } else {
                        stats.ntp.last_tx_ms = tx_elapsed_ms;
                    }
                }

                // Log toutes les 60 secondes
                if current_requests % 60 == 0 {
                    stats_clone.log_stats();
                }
            }
        });

        let mut buffer = [0u8; NtpPacket::SIZE];

        loop {
            // Vérifier si l'arrêt a été demandé
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Shutdown signal received, stopping NTP server...");
                break;
            }

            match self.handle_request(&socket, &mut buffer) {
                Ok(_) => {}
                Err(e) => {
                    // Ignorer les timeouts (normaux pour pouvoir vérifier shutdown)
                    if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
                        if io_error.kind() == std::io::ErrorKind::WouldBlock
                            || io_error.kind() == std::io::ErrorKind::TimedOut {
                            continue;
                        }
                    }
                    error!("Error handling request: {:#}", e);
                    self.stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        info!("NTP server stopped");
        Ok(())
    }

    /// Gère une requête NTP
    fn handle_request(&self, socket: &UdpSocket, buffer: &mut [u8]) -> Result<()> {
        // Réception du paquet
        let (size, client_addr) = socket.recv_from(buffer)?;

        // TIMESTAMP T2: Moment de réception (le plus tôt possible après recv_from)
        let receive_time = self.clock.now();

        self.stats.requests_received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Extraction de l'IP du client
        let client_ip = client_addr.ip();

        // Vérification du filtre IP
        if !self.ip_filter.is_allowed(client_ip) {
            debug!("Request from {} rejected by IP filter", client_addr);
            self.stats.requests_rejected.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }

        // Vérification du rate limiting
        if let Some(ref limiter) = self.rate_limiter {
            if !limiter.check_rate_limit(client_ip) {
                warn!("Request from {} rejected by rate limiter", client_addr);
                self.stats.requests_rejected.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(());
            }
        }

        // Parse du paquet NTP
        let request_packet = match NtpPacket::from_bytes(&buffer[..size]) {
            Ok(packet) => packet,
            Err(e) => {
                warn!("Failed to parse NTP packet from {}: {}", client_addr, e);
                self.stats.requests_rejected.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(());
            }
        };

        // Validation du paquet
        if let Err(e) = PacketValidator::validate_request(&request_packet) {
            warn!("Invalid NTP request from {}: {}", client_addr, e);
            self.stats.requests_rejected.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(());
        }

        if self.config.logging.log_requests {
            debug!(
                "NTP request from {}: version={}, mode={:?}, stratum={}",
                client_addr, request_packet.version, request_packet.mode, request_packet.stratum
            );
        }

        // Création de la réponse
        let response = self.create_response(&request_packet, receive_time);

        // TIMESTAMP T3: Moment de transmission (le plus tard possible avant send_to)
        let transmit_time = self.clock.now();
        let mut response = response;
        response.transmit_timestamp = transmit_time;

        // Sérialisation et envoi
        let response_bytes = response.to_bytes();
        socket.send_to(&response_bytes, client_addr)?;

        self.stats.requests_processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Mettre à jour les stats partagées
        let total_requests = self.stats.requests_processed.load(std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut stats) = self.shared_stats.write() {
            stats.ntp.requests_total = total_requests;
            stats.ntp.last_tx_ms = 0; // TX vient de se produire

            // Mettre à jour clock info
            let timestamp = self.clock.now();
            stats.clock.current_timestamp = timestamp.seconds() as u64;
            stats.clock.current_fraction_ns = ((timestamp.fraction() as u64 * 1_000_000_000) >> 32) as u32;
            stats.clock.stratum = self.clock.stratum();
            stats.clock.reference_id = String::from_utf8_lossy(&self.clock.reference_id()).to_string();
            stats.clock.precision = self.clock.precision();
        }

        if self.config.logging.log_requests {
            debug!("NTP response sent to {}", client_addr);
        }

        Ok(())
    }

    /// Crée une réponse NTP
    fn create_response(&self, request: &NtpPacket, receive_time: NtpTimestamp) -> NtpPacket {
        let mut response = NtpPacket::new_server_response();

        // Leap Indicator: copier depuis la source d'horloge ou mettre à 0
        response.leap_indicator = LeapIndicator::NoWarning;

        // Version: copier depuis la requête
        response.version = request.version;

        // Mode: Server (4)
        response.mode = NtpMode::Server;

        // Stratum: obtenir depuis la source d'horloge
        response.stratum = self.clock.stratum();

        // Poll: copier depuis la requête
        response.poll = request.poll;

        // Precision: obtenir depuis la source d'horloge
        response.precision = self.clock.precision();

        // Root delay et dispersion (0 pour stratum 1)
        response.root_delay = 0;
        response.root_dispersion = 0;

        // Reference identifier: obtenir depuis la source d'horloge
        let ref_id_bytes = self.clock.reference_id();
        response.reference_identifier = u32::from_be_bytes(ref_id_bytes);

        // Reference timestamp: temps de la dernière synchronisation
        // Pour un serveur stratum 1, c'est le temps actuel
        response.reference_timestamp = self.clock.now();

        // Originate timestamp (T1): copier le transmit timestamp de la requête
        response.originate_timestamp = request.transmit_timestamp;

        // Receive timestamp (T2): temps de réception capturé plus tôt
        response.receive_timestamp = receive_time;

        // Transmit timestamp (T3): sera rempli juste avant l'envoi
        response.transmit_timestamp = NtpTimestamp::default();

        response
    }

    /// Retourne les statistiques du serveur
    #[allow(dead_code)]
    pub fn stats(&self) -> &Arc<ServerStats> {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;
    use crate::packet::NtpMode;

    #[test]
    fn test_create_response() {
        let config = Config::default();
        let clock = Arc::new(SystemClock::new());
        let server = NtpServer::new(config, clock);

        let mut request = NtpPacket::new_server_response();
        request.mode = NtpMode::Client;
        request.version = 4;
        request.transmit_timestamp = NtpTimestamp::from_seconds_and_nanos(3_900_000_000, 0);

        let receive_time = NtpTimestamp::from_seconds_and_nanos(3_900_000_001, 0);
        let response = server.create_response(&request, receive_time);

        assert_eq!(response.version, 4);
        assert_eq!(response.mode, NtpMode::Server);
        assert_eq!(response.originate_timestamp, request.transmit_timestamp);
        assert_eq!(response.receive_timestamp, receive_time);
    }
}
