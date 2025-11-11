use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{warn, debug};

/// Gestionnaire de rate limiting par IP
pub struct RateLimiter {
    /// Map: IP -> état du rate limiting
    limits: Arc<RwLock<HashMap<IpAddr, RateLimitState>>>,

    /// Nombre maximum de requêtes par seconde
    max_requests_per_second: u32,

    /// Fenêtre de temps pour le nettoyage des anciennes entrées
    cleanup_interval: Duration,

    /// Dernier nettoyage
    last_cleanup: Arc<RwLock<Instant>>,
}

#[derive(Debug, Clone)]
struct RateLimitState {
    /// Nombre de requêtes dans la fenêtre actuelle
    request_count: u32,

    /// Début de la fenêtre actuelle
    window_start: Instant,

    /// Dernière requête vue
    last_request: Instant,
}

impl RateLimiter {
    pub fn new(max_requests_per_second: u32) -> Self {
        RateLimiter {
            limits: Arc::new(RwLock::new(HashMap::new())),
            max_requests_per_second,
            cleanup_interval: Duration::from_secs(60),
            last_cleanup: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Vérifie si une requête depuis cette IP est autorisée
    /// Retourne true si autorisé, false si rate limited
    pub fn check_rate_limit(&self, ip: IpAddr) -> bool {
        let now = Instant::now();

        // Nettoyage périodique des anciennes entrées
        self.cleanup_old_entries(now);

        let mut limits = match self.limits.write() {
            Ok(guard) => guard,
            Err(_) => {
                warn!("Failed to acquire rate limiter write lock");
                return true; // Fail open en cas d'erreur de lock
            }
        };

        let state = limits.entry(ip).or_insert_with(|| RateLimitState {
            request_count: 0,
            window_start: now,
            last_request: now,
        });

        // Si plus d'une seconde s'est écoulée, réinitialiser la fenêtre
        if now.duration_since(state.window_start) >= Duration::from_secs(1) {
            state.request_count = 1;
            state.window_start = now;
            state.last_request = now;
            return true;
        }

        // Incrémenter le compteur
        state.request_count += 1;
        state.last_request = now;

        if state.request_count > self.max_requests_per_second {
            debug!(
                "Rate limit exceeded for IP {}: {} requests/sec",
                ip, state.request_count
            );
            return false;
        }

        true
    }

    /// Nettoie les entrées inactives depuis plus de 60 secondes
    fn cleanup_old_entries(&self, now: Instant) {
        let mut last_cleanup = match self.last_cleanup.write() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        // Nettoyer seulement toutes les 60 secondes
        if now.duration_since(*last_cleanup) < self.cleanup_interval {
            return;
        }

        if let Ok(mut limits) = self.limits.write() {
            let inactive_threshold = Duration::from_secs(60);
            limits.retain(|_, state| {
                now.duration_since(state.last_request) < inactive_threshold
            });

            debug!("Cleaned up rate limiter, {} IPs tracked", limits.len());
        }

        *last_cleanup = now;
    }

    /// Retourne les statistiques du rate limiter
    #[allow(dead_code)]
    pub fn stats(&self) -> RateLimiterStats {
        let limits = self.limits.read().unwrap();
        RateLimiterStats {
            tracked_ips: limits.len(),
        }
    }
}

#[allow(dead_code)]
pub struct RateLimiterStats {
    pub tracked_ips: usize,
}

/// Gestionnaire de listes blanches/noires IP
pub struct IpFilter {
    whitelist: Vec<IpAddr>,
    blacklist: Vec<IpAddr>,
}

impl IpFilter {
    pub fn new(whitelist: Vec<String>, blacklist: Vec<String>) -> Self {
        let whitelist: Vec<IpAddr> = whitelist
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        let blacklist: Vec<IpAddr> = blacklist
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        IpFilter {
            whitelist,
            blacklist,
        }
    }

    /// Vérifie si une IP est autorisée
    pub fn is_allowed(&self, ip: IpAddr) -> bool {
        // Vérifier d'abord la blacklist
        if self.blacklist.contains(&ip) {
            debug!("IP {} blocked by blacklist", ip);
            return false;
        }

        // Si whitelist vide, tout est autorisé (sauf blacklist)
        if self.whitelist.is_empty() {
            return true;
        }

        // Si whitelist non vide, l'IP doit être dedans
        let allowed = self.whitelist.contains(&ip);
        if !allowed {
            debug!("IP {} not in whitelist", ip);
        }
        allowed
    }
}

/// Validation des paquets NTP
pub struct PacketValidator;

impl PacketValidator {
    /// Valide un paquet NTP reçu
    pub fn validate_request(packet: &crate::packet::NtpPacket) -> Result<(), ValidationError> {
        // Vérifier la version NTP (accepter v1 à v4 pour compatibilité)
        if packet.version < 1 || packet.version > 4 {
            return Err(ValidationError::InvalidVersion(packet.version));
        }

        // Vérifier le mode (doit être client = 3)
        if packet.mode != crate::packet::NtpMode::Client {
            return Err(ValidationError::InvalidMode);
        }

        // Vérifier que le transmit timestamp n'est pas nul
        if packet.transmit_timestamp.0 == 0 {
            return Err(ValidationError::ZeroTransmitTimestamp);
        }

        // Vérifier le stratum (0 = kiss-o-death, >= 16 = non synchronisé)
        if packet.stratum >= 16 {
            return Err(ValidationError::InvalidStratum(packet.stratum));
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Invalid NTP version: {0}")]
    InvalidVersion(u8),

    #[error("Invalid NTP mode (expected client)")]
    InvalidMode,

    #[error("Zero transmit timestamp")]
    ZeroTransmitTimestamp,

    #[error("Invalid stratum: {0}")]
    InvalidStratum(u8),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(10);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        // Devrait autoriser les 10 premières requêtes
        for _ in 0..10 {
            assert!(limiter.check_rate_limit(ip));
        }

        // La 11ème doit être bloquée
        assert!(!limiter.check_rate_limit(ip));
    }

    #[test]
    fn test_ip_filter_blacklist() {
        let filter = IpFilter::new(
            vec![],
            vec!["192.168.1.100".to_string()],
        );

        let blocked_ip = "192.168.1.100".parse().unwrap();
        let allowed_ip = "192.168.1.101".parse().unwrap();

        assert!(!filter.is_allowed(blocked_ip));
        assert!(filter.is_allowed(allowed_ip));
    }

    #[test]
    fn test_ip_filter_whitelist() {
        let filter = IpFilter::new(
            vec!["192.168.1.100".to_string()],
            vec![],
        );

        let allowed_ip = "192.168.1.100".parse().unwrap();
        let blocked_ip = "192.168.1.101".parse().unwrap();

        assert!(filter.is_allowed(allowed_ip));
        assert!(!filter.is_allowed(blocked_ip));
    }
}
