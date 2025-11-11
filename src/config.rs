use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use anyhow::{Context, Result};

/// Configuration du serveur NTP
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    /// Configuration du serveur
    pub server: ServerConfig,

    /// Configuration de la source d'horloge
    pub clock: ClockConfig,

    /// Configuration de sécurité
    pub security: SecurityConfig,

    /// Configuration des logs
    pub logging: LoggingConfig,

    /// Configuration du serveur web
    #[serde(default)]
    pub webserver: WebServerConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerConfig {
    /// Adresse d'écoute (ex: "0.0.0.0:123")
    pub bind_address: String,

    /// Stratum du serveur (1-15, 1 = source primaire)
    /// Si clock_source = "gps", ce sera automatiquement 1 quand synchronisé
    #[serde(default = "default_stratum")]
    pub stratum: u8,

    /// Précision annoncée en log2 secondes (ex: -20 = ~1µs)
    #[serde(default = "default_precision")]
    pub precision: i8,

    /// Intervalle de polling recommandé (en log2 secondes)
    #[serde(default = "default_poll")]
    pub poll_interval: i8,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClockConfig {
    /// Source d'horloge: "system" ou "gps"
    #[serde(default = "default_clock_source")]
    pub source: String,

    /// Configuration GPS (utilisé si source = "gps")
    pub gps: Option<GpsConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GpsConfig {
    /// Activer le module GPS (si false, le GPS ne sera pas initialisé)
    #[serde(default = "default_gps_enabled")]
    pub enabled: bool,

    /// Port série du module GPS (ex: "COM9" sur Windows, "/dev/ttyUSB0" sur Linux)
    pub serial_port: String,

    /// Baud rate (généralement 9600 pour NMEA)
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,

    /// Timeout de synchronisation GPS en secondes
    /// Si aucune donnée GPS valide n'est reçue pendant ce délai,
    /// le serveur passe en mode non-synchronisé
    #[serde(default = "default_gps_timeout")]
    pub sync_timeout: u64,

    /// Nombre minimum de satellites requis
    #[serde(default = "default_min_satellites")]
    pub min_satellites: u8,

    /// Activer la détection PPS via CTS (Pulse Per Second)
    /// Le signal PPS est détecté via la ligne CTS du port série
    #[serde(default = "default_pps_enabled")]
    pub pps_enabled: bool,

    /// Pin GPIO pour PPS (Linux/Raspberry Pi uniquement, ex: 18 pour GPIO18)
    /// Optionnel : utilisé uniquement pour PPS kernel Linux avancé
    pub pps_gpio_pin: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SecurityConfig {
    /// Activer le rate limiting
    #[serde(default = "default_true")]
    pub enable_rate_limiting: bool,

    /// Nombre maximum de requêtes par seconde par IP
    #[serde(default = "default_max_requests_per_second")]
    pub max_requests_per_second: u32,

    /// Liste blanche d'adresses IP (vide = toutes autorisées)
    #[serde(default)]
    pub ip_whitelist: Vec<String>,

    /// Liste noire d'adresses IP
    #[serde(default)]
    pub ip_blacklist: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LoggingConfig {
    /// Niveau de log: "trace", "debug", "info", "warn", "error"
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Activer les logs de chaque requête
    #[serde(default = "default_false")]
    pub log_requests: bool,

    /// Fichier de log (vide = stdout uniquement)
    pub log_file: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WebServerConfig {
    /// Port du serveur web (interface de monitoring)
    #[serde(default = "default_web_port")]
    pub port: u16,

    /// Adresse d'écoute du serveur web
    #[serde(default = "default_web_bind_address")]
    pub bind_address: String,
}

// Fonctions par défaut pour serde
fn default_stratum() -> u8 { 2 }
fn default_precision() -> i8 { -20 }
fn default_poll() -> i8 { 6 }
fn default_clock_source() -> String { "system".to_string() }
fn default_gps_enabled() -> bool { true }
fn default_baud_rate() -> u32 { 9600 }
fn default_gps_timeout() -> u64 { 30 }
fn default_min_satellites() -> u8 { 4 }
fn default_pps_enabled() -> bool { true }
fn default_true() -> bool { true }
fn default_false() -> bool { false }
fn default_max_requests_per_second() -> u32 { 100 }
fn default_log_level() -> String { "info".to_string() }
fn default_web_port() -> u16 { 8080 }
fn default_web_bind_address() -> String { "0.0.0.0".to_string() }

impl Default for Config {
    fn default() -> Self {
        Config {
            server: ServerConfig {
                bind_address: "0.0.0.0:123".to_string(),
                stratum: 2,
                precision: -20,
                poll_interval: 6,
            },
            clock: ClockConfig {
                source: "system".to_string(),
                gps: None,
            },
            security: SecurityConfig {
                enable_rate_limiting: true,
                max_requests_per_second: 100,
                ip_whitelist: vec![],
                ip_blacklist: vec![],
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                log_requests: false,
                log_file: None,
            },
            webserver: WebServerConfig {
                port: 8080,
                bind_address: "0.0.0.0".to_string(),
            },
        }
    }
}

impl Default for WebServerConfig {
    fn default() -> Self {
        WebServerConfig {
            port: 8080,
            bind_address: "0.0.0.0".to_string(),
        }
    }
}

impl Config {
    /// Charge la configuration depuis un fichier TOML
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .context("Failed to read config file")?;

        let config: Config = toml::from_str(&content)
            .context("Failed to parse config file")?;

        config.validate()?;
        Ok(config)
    }

    /// Sauvegarde la configuration dans un fichier TOML
    pub fn to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        fs::write(path.as_ref(), content)
            .context("Failed to write config file")?;

        Ok(())
    }

    /// Valide la configuration
    fn validate(&self) -> Result<()> {
        // Validation du stratum
        if self.server.stratum == 0 || self.server.stratum > 15 {
            anyhow::bail!("Invalid stratum: must be between 1 and 15");
        }

        // Validation de la source d'horloge
        if self.clock.source != "system" && self.clock.source != "gps" {
            anyhow::bail!("Invalid clock source: must be 'system' or 'gps'");
        }

        // Si source GPS, vérifier la config GPS
        if self.clock.source == "gps" && self.clock.gps.is_none() {
            anyhow::bail!("GPS clock source selected but no GPS configuration provided");
        }

        Ok(())
    }

    /// Crée un fichier de configuration exemple
    pub fn create_example_config<P: AsRef<Path>>(path: P) -> Result<()> {
        // Détecter la plateforme pour mettre des valeurs par défaut adaptées
        #[cfg(target_os = "windows")]
        let (default_port, default_log) = ("COM9".to_string(), Some("pendulum.log".to_string()));

        #[cfg(target_os = "linux")]
        let (default_port, default_log) = ("/dev/ttyUSB0".to_string(), Some("/var/log/pendulum.log".to_string()));

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let (default_port, default_log) = ("/dev/ttyUSB0".to_string(), Some("/var/log/pendulum.log".to_string()));

        let example_config = Config {
            server: ServerConfig {
                bind_address: "0.0.0.0:123".to_string(),
                stratum: 1,
                precision: -20,
                poll_interval: 6,
            },
            clock: ClockConfig {
                source: "gps".to_string(),
                gps: Some(GpsConfig {
                    enabled: true,
                    serial_port: default_port,
                    baud_rate: 9600,
                    sync_timeout: 30,
                    min_satellites: 4,
                    pps_enabled: true,
                    pps_gpio_pin: Some(18),
                }),
            },
            security: SecurityConfig {
                enable_rate_limiting: true,
                max_requests_per_second: 100,
                ip_whitelist: vec![],
                ip_blacklist: vec![],
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                log_requests: true,
                log_file: default_log,
            },
            webserver: WebServerConfig {
                port: 8080,
                bind_address: "0.0.0.0".to_string(),
            },
        };

        example_config.to_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.bind_address, "0.0.0.0:123");
        assert_eq!(config.clock.source, "system");
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config::default();

        // Stratum invalide
        config.server.stratum = 0;
        assert!(config.validate().is_err());

        config.server.stratum = 16;
        assert!(config.validate().is_err());

        // Stratum valide
        config.server.stratum = 1;
        assert!(config.validate().is_ok());
    }
}
