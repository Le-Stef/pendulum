use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// Informations sur un satellite GPS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteInfo {
    /// Numéro PRN du satellite (1-32 GPS, 33+ pour autres constellations)
    pub prn: u8,

    /// Élévation en degrés (0-90, 0=horizon, 90=zénith)
    pub elevation: u8,

    /// Azimut en degrés (0-359, 0=Nord, 90=Est, 180=Sud, 270=Ouest)
    pub azimuth: u16,

    /// Signal-to-Noise Ratio en dB-Hz (0-99, 0=pas de signal)
    pub snr: u8,

    /// Constellation (GPS, GLONASS, Galileo, BeiDou)
    pub constellation: String,
}

/// Statistiques partagées entre le serveur NTP, GPS et l'interface web
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStats {
    /// Statistiques GPS
    pub gps: GpsStats,

    /// Statistiques NTP
    pub ntp: NtpStats,

    /// Informations horloge
    pub clock: ClockInfo,

    /// Liste des satellites en vue
    pub satellites: Vec<SatelliteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpsStats {
    /// GPS connecté et fonctionnel
    pub connected: bool,

    /// Nombre de satellites visibles
    pub satellites: u8,

    /// Qualité du signal (0-10)
    pub signal_quality: u8,

    /// Dernière synchronisation GPS (secondes depuis démarrage)
    pub last_sync_secs: Option<u64>,

    /// Nombre total de trames NMEA reçues
    pub nmea_sentences: u64,

    /// PPS actif
    pub pps_active: bool,

    /// Nombre de pulses PPS reçus
    pub pps_count: u64,

    /// Dernière activité RX (millisecondes depuis)
    pub last_rx_ms: u64,

    /// Offset PPS actuel (secondes)
    pub pps_offset: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtpStats {
    /// Nombre total de requêtes traitées
    pub requests_total: u64,

    /// Nombre de requêtes traitées dans la dernière seconde
    pub requests_per_second: u32,

    /// Nombre de clients actifs (IPs uniques dans les 60 dernières secondes)
    pub active_clients: usize,

    /// Dernière activité TX (millisecondes depuis)
    pub last_tx_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockInfo {
    /// Stratum NTP
    pub stratum: u8,

    /// Identifiant de référence (ex: "GPS", "LOCL")
    pub reference_id: String,

    /// Précision en log2 secondes
    pub precision: i8,

    /// Timestamp NTP actuel (secondes depuis epoch NTP 1900)
    pub current_timestamp: u64,

    /// Partie fractionnaire (en nanosecondes)
    pub current_fraction_ns: u32,
}

/// Gestionnaire de statistiques partagé via Arc<RwLock>
pub struct StatsManager {
    stats: Arc<RwLock<ServerStats>>,
}

impl StatsManager {
    pub fn new() -> Self {
        let stats = ServerStats {
            gps: GpsStats {
                connected: false,
                satellites: 0,
                signal_quality: 0,
                last_sync_secs: None,
                nmea_sentences: 0,
                pps_active: false,
                pps_count: 0,
                last_rx_ms: 0,
                pps_offset: None,
            },
            ntp: NtpStats {
                requests_total: 0,
                requests_per_second: 0,
                active_clients: 0,
                last_tx_ms: 0,
            },
            clock: ClockInfo {
                stratum: 16,
                reference_id: "INIT".to_string(),
                precision: -20,
                current_timestamp: 0,
                current_fraction_ns: 0,
            },
            satellites: Vec::new(),
        };

        StatsManager {
            stats: Arc::new(RwLock::new(stats)),
        }
    }

    /// Retourne un clone de l'Arc pour partager entre threads
    pub fn clone_arc(&self) -> Arc<RwLock<ServerStats>> {
        Arc::clone(&self.stats)
    }

    /// Lit les statistiques actuelles
    #[allow(dead_code)]
    pub fn get(&self) -> ServerStats {
        self.stats.read().unwrap().clone()
    }

    /// Met à jour les statistiques GPS
    #[allow(dead_code)]
    pub fn update_gps<F>(&self, f: F)
    where
        F: FnOnce(&mut GpsStats),
    {
        if let Ok(mut stats) = self.stats.write() {
            f(&mut stats.gps);
        }
    }

    /// Met à jour les statistiques NTP
    #[allow(dead_code)]
    pub fn update_ntp<F>(&self, f: F)
    where
        F: FnOnce(&mut NtpStats),
    {
        if let Ok(mut stats) = self.stats.write() {
            f(&mut stats.ntp);
        }
    }

    /// Met à jour les informations d'horloge
    pub fn update_clock<F>(&self, f: F)
    where
        F: FnOnce(&mut ClockInfo),
    {
        if let Ok(mut stats) = self.stats.write() {
            f(&mut stats.clock);
        }
    }

    /// Met à jour la liste des satellites
    #[allow(dead_code)]
    pub fn update_satellites(&self, satellites: Vec<SatelliteInfo>) {
        if let Ok(mut stats) = self.stats.write() {
            stats.satellites = satellites;
        }
    }
}

impl Default for StatsManager {
    fn default() -> Self {
        Self::new()
    }
}
