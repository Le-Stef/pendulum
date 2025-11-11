/*!
Module de lecture GPS avec reconnexion automatique et support PPS via CTS

Ce module gère la connexion au module GPS/GNSS, lit les trames NMEA,
détecte le signal PPS via la ligne CTS du port série, et met à jour
l'horloge GPS du serveur NTP.

Architecture robuste :
- Thread séparé pour ne jamais bloquer le serveur NTP
- Reconnexion automatique en cas de déconnexion
- Gestion d'erreurs complète sans panic
- Logging détaillé des événements
*/

use crate::clock::GpsNmeaClock;
use crate::config::GpsConfig;
use crate::packet::NtpTimestamp;
use crate::stats::{SatelliteInfo, ServerStats};
use chrono::NaiveDateTime;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Gestionnaire de lecture GPS
pub struct GpsReader {
    config: GpsConfig,
    clock: Arc<GpsNmeaClock>,
    stats: Arc<std::sync::RwLock<ServerStats>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    start_time: Instant,
}

impl GpsReader {
    /// Crée un nouveau lecteur GPS
    pub fn new(
        config: GpsConfig,
        clock: Arc<GpsNmeaClock>,
        stats: Arc<std::sync::RwLock<ServerStats>>,
    ) -> Self {
        GpsReader {
            config,
            clock,
            stats,
            running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            start_time: Instant::now(),
        }
    }

    /// Démarre le thread de lecture GPS
    /// Le thread tourne indéfiniment avec reconnexion automatique
    pub fn start(self) -> std::thread::JoinHandle<()> {
        info!("Starting GPS reader thread");
        info!("  Port: {}", self.config.serial_port);
        info!("  Baud rate: {}", self.config.baud_rate);
        info!("  PPS via CTS: {}", self.config.pps_enabled);
        info!("  Min satellites: {}", self.config.min_satellites);

        std::thread::spawn(move || {
            let mut reconnect_delay = Duration::from_secs(5);
            let max_reconnect_delay = Duration::from_secs(60);

            while self.running.load(std::sync::atomic::Ordering::Relaxed) {
                match self.run_reader() {
                    Ok(_) => {
                        // Connexion réussie puis terminée normalement
                        info!("GPS reader stopped normally");
                        break;
                    }
                    Err(e) => {
                        error!("GPS reader error: {:#}", e);
                        error!("Reconnecting in {:?}...", reconnect_delay);

                        // Attendre avant de reconnecter
                        std::thread::sleep(reconnect_delay);

                        // Augmenter progressivement le délai (exponential backoff)
                        reconnect_delay = std::cmp::min(
                            reconnect_delay * 2,
                            max_reconnect_delay,
                        );
                    }
                }
            }

            info!("GPS reader thread terminated");
        })
    }

    /// Arrête le thread GPS proprement
    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Boucle principale de lecture GPS
    fn run_reader(&self) -> anyhow::Result<()> {
        info!("Opening GPS serial port: {}", self.config.serial_port);

        // Ouvrir le port série
        let mut port = serialport::new(&self.config.serial_port, self.config.baud_rate)
            .timeout(Duration::from_millis(100))
            .open()?;

        // Configuration des lignes de contrôle
        port.write_request_to_send(true)?;
        port.write_data_terminal_ready(true)?;
        port.clear(serialport::ClearBuffer::All)?;

        info!("GPS serial port opened successfully");

        // Marquer GPS comme connecté dans les stats
        if let Ok(mut stats) = self.stats.write() {
            stats.gps.connected = true;
        }

        // État de lecture
        let mut buffer = String::new();
        let mut read_buf = [0u8; 512];
        let mut last_cts = port.read_clear_to_send()?;
        let mut last_pps_pulse = Instant::now();
        let mut pps_count: u64 = 0;
        let mut nmea_count: u64 = 0;
        let mut last_stats_log = Instant::now();
        let mut last_rx = Instant::now();

        // Pour la correction PPS : stocker le dernier timestamp GPS reçu
        let mut last_gps_timestamp: Option<NtpTimestamp> = None;

        // Pour le skyplot : stocker les satellites en vue
        let mut satellites_in_view: Vec<SatelliteInfo> = Vec::new();
        let mut last_satellite_update = Instant::now();

        // Boucle de lecture
        while self.running.load(std::sync::atomic::Ordering::Relaxed) {
            // Lecture des données NMEA
            match port.read(&mut read_buf) {
                Ok(n) if n > 0 => {
                    last_rx = Instant::now();
                    let s = String::from_utf8_lossy(&read_buf[..n]);
                    buffer.push_str(&s);

                    // Mettre à jour last_rx_ms dans les stats
                    if let Ok(mut stats) = self.stats.write() {
                        stats.gps.last_rx_ms = 0; // Donnée juste reçue
                    }

                    // Traitement ligne par ligne
                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer.drain(..=pos).collect::<String>();
                        let trimmed = line.trim();

                        // Log toutes les trames pour debug (seulement les premières 80 chars)
                        if trimmed.len() > 0 {
                            let preview = if trimmed.len() > 80 { &trimmed[..80] } else { trimmed };
                            debug!("NMEA: {}", preview);
                        }

                        // Parser les satellites (GPGSV)
                        if let Some(sats) = self.parse_gpgsv(trimmed) {
                            debug!("GPGSV parsed: {} satellites in this sentence", sats.len());

                            // Mettre à jour ou ajouter les satellites
                            for sat in sats {
                                // Remplacer si satellite existe déjà, sinon ajouter
                                if let Some(existing) = satellites_in_view.iter_mut().find(|s| s.prn == sat.prn) {
                                    *existing = sat;
                                } else {
                                    satellites_in_view.push(sat);
                                }
                            }

                            // Mettre à jour les stats toutes les 2 secondes (éviter trop de writes)
                            if last_satellite_update.elapsed() > Duration::from_secs(2) {
                                debug!("Updating satellite stats: {} satellites total", satellites_in_view.len());
                                if let Ok(mut stats) = self.stats.write() {
                                    stats.satellites = satellites_in_view.clone();
                                }
                                last_satellite_update = Instant::now();
                            }
                        }

                        // Parser le temps GPS (GPRMC)
                        if let Some(timestamp) = self.process_nmea_sentence(trimmed) {
                            nmea_count += 1;
                            // Stocker le dernier timestamp GPS reçu
                            last_gps_timestamp = Some(timestamp);

                            // Mettre à jour les stats
                            if let Ok(mut stats) = self.stats.write() {
                                stats.gps.nmea_sentences = nmea_count;
                                stats.gps.last_sync_secs = Some(self.start_time.elapsed().as_secs());
                            }
                        }
                    }
                }
                Ok(_) => {
                    // Pas de données, continuer
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // Timeout normal, continuer
                }
                Err(e) => {
                    return Err(e.into());
                }
            }

            // Détection du signal PPS via CTS (si activé)
            if self.config.pps_enabled {
                match port.read_clear_to_send() {
                    Ok(cts) if cts != last_cts => {
                        last_cts = cts;
                        if cts {
                            // Front montant = pulse PPS
                            let now = Instant::now();
                            let interval = now.duration_since(last_pps_pulse);
                            last_pps_pulse = now;
                            pps_count += 1;

                            // Vérifier que l'intervalle est proche de 1 seconde
                            let interval_secs = interval.as_secs_f64();
                            if (0.95..=1.05).contains(&interval_secs) {
                                debug!(
                                    "PPS pulse detected (#{}) - interval: {:.6}s",
                                    pps_count, interval_secs
                                );

                                // Si on a un timestamp GPS précédent, calculer l'offset PPS
                                // Le PPS actuel correspond au timestamp GPS + 1 seconde
                                if let Some(prev_gps_ts) = last_gps_timestamp {
                                    // Le PPS correspond au début de la seconde suivante
                                    let gps_second_boundary = NtpTimestamp::from_seconds_and_nanos(
                                        prev_gps_ts.seconds() as u64 + 1,
                                        0,
                                    );

                                    // Mettre à jour l'offset PPS dans l'horloge
                                    self.clock.update_pps_offset(now, gps_second_boundary);

                                    debug!(
                                        "PPS offset updated for GPS second {}",
                                        gps_second_boundary.seconds()
                                    );

                                    // Mettre à jour les stats PPS
                                    if let Ok(mut stats) = self.stats.write() {
                                        stats.gps.pps_count = pps_count;
                                        stats.gps.pps_active = true;
                                        stats.gps.pps_offset = self.clock.get_pps_offset();
                                    }
                                }
                            } else if pps_count > 1 {
                                // Premier pulse peut avoir un intervalle bizarre
                                warn!(
                                    "PPS interval out of range: {:.6}s (expected ~1.0s)",
                                    interval_secs
                                );
                            }

                            // Mettre à jour le compte PPS même si l'intervalle est bizarre
                            if let Ok(mut stats) = self.stats.write() {
                                stats.gps.pps_count = pps_count;
                            }
                        }
                    }
                    Ok(_) => {
                        // Pas de changement CTS
                    }
                    Err(e) => {
                        warn!("Failed to read CTS status: {}", e);
                    }
                }
            }

            // Mettre à jour last_rx_ms périodiquement
            let rx_elapsed_ms = last_rx.elapsed().as_millis() as u64;
            if let Ok(mut stats) = self.stats.write() {
                stats.gps.last_rx_ms = rx_elapsed_ms;
            }

            // Log des stats périodiquement
            if last_stats_log.elapsed() > Duration::from_secs(60) {
                info!(
                    "GPS stats: {} NMEA sentences, {} PPS pulses processed",
                    nmea_count, pps_count
                );
                last_stats_log = Instant::now();
            }
        }

        // Marquer GPS comme déconnecté à la sortie
        if let Ok(mut stats) = self.stats.write() {
            stats.gps.connected = false;
            stats.gps.pps_active = false;
        }

        Ok(())
    }

    /// Traite une trame NMEA et met à jour l'horloge si valide
    /// Retourne le timestamp GPS si la trame a été traitée avec succès
    fn process_nmea_sentence(&self, sentence: &str) -> Option<NtpTimestamp> {
        // On traite principalement GPRMC qui contient date + heure + statut
        if sentence.starts_with("$GPRMC") || sentence.starts_with("$GNRMC") {
            if let Some((timestamp, satellites)) = self.parse_gprmc(sentence) {
                // Mettre à jour l'horloge GPS
                self.clock.update_gps_time(timestamp, satellites);

                debug!(
                    "GPS time synchronized: {} seconds since NTP epoch, {} satellites",
                    timestamp.seconds(),
                    satellites
                );

                // Mettre à jour les stats satellites
                if let Ok(mut stats) = self.stats.write() {
                    stats.gps.satellites = satellites;
                    // Signal quality basé sur le nombre de satellites (0-10)
                    stats.gps.signal_quality = (satellites.min(10)) as u8;
                }

                return Some(timestamp);
            }
        }

        // On peut aussi traiter GPGGA pour plus d'infos sur les satellites
        if sentence.starts_with("$GPGGA") || sentence.starts_with("$GNGGA") {
            if let Some(sat_count) = self.parse_gpgga_satellites(sentence) {
                debug!("GPS satellites in view: {}", sat_count);

                // Mettre à jour les stats avec le vrai compte de satellites
                if let Ok(mut stats) = self.stats.write() {
                    stats.gps.satellites = sat_count;
                    stats.gps.signal_quality = (sat_count.min(10)) as u8;
                }
            }
        }

        None
    }

    /// Parse une trame GPRMC et extrait le timestamp NTP
    fn parse_gprmc(&self, sentence: &str) -> Option<(NtpTimestamp, u8)> {
        let fields: Vec<&str> = sentence.split(',').collect();

        // Vérifier format minimal GPRMC
        if fields.len() < 10 {
            return None;
        }

        // Champ 2 : Statut (A = valide, V = invalide)
        if fields[2] != "A" {
            debug!("GPS fix not valid (status: {})", fields[2]);
            return None;
        }

        // Champ 1 : Heure UTC (hhmmss.sss)
        let time_str = fields[1];
        if time_str.len() < 6 {
            return None;
        }

        // Champ 9 : Date (ddmmyy)
        let date_str = fields[9];
        if date_str.len() != 6 {
            return None;
        }

        // Parser avec chrono pour validation
        let datetime_str = format!(
            "20{}-{}-{} {}:{}:{}",
            &date_str[4..6], // année
            &date_str[2..4], // mois
            &date_str[0..2], // jour
            &time_str[0..2], // heure
            &time_str[2..4], // minute
            &time_str[4..6]  // seconde
        );

        let parsed = NaiveDateTime::parse_from_str(&datetime_str, "%Y-%m-%d %H:%M:%S").ok()?;

        // Convertir en timestamp NTP (secondes depuis 1900-01-01)
        let unix_timestamp = parsed.and_utc().timestamp() as u64;
        let ntp_timestamp_secs = unix_timestamp + 2_208_988_800; // NTP epoch offset

        // Extraire les fractions de seconde si présentes
        let subsec_nanos = if time_str.len() > 7 && time_str.chars().nth(6) == Some('.') {
            let frac_str = &time_str[7..];
            let frac_value: u32 = frac_str.parse().unwrap_or(0);
            // Convertir en nanosecondes (assuming 3 digits = milliseconds)
            frac_value * 1_000_000
        } else {
            0
        };

        let ntp_timestamp = NtpTimestamp::from_seconds_and_nanos(ntp_timestamp_secs, subsec_nanos);

        // Estimer le nombre de satellites (GPRMC ne le donne pas directement)
        // On utilise une valeur par défaut, GPGGA nous donnera la vraie valeur
        let satellites = self.config.min_satellites;

        Some((ntp_timestamp, satellites))
    }

    /// Parse une trame GPGGA pour extraire le nombre de satellites
    fn parse_gpgga_satellites(&self, sentence: &str) -> Option<u8> {
        let fields: Vec<&str> = sentence.split(',').collect();

        if fields.len() < 8 {
            return None;
        }

        // Champ 7 : Nombre de satellites
        fields[7].parse().ok()
    }

    /// Parse une trame GPGSV (GPS Satellites in View) pour extraire positions satellites
    /// Format: $GPGSV,total_msgs,msg_num,total_sats,sat1_prn,sat1_elev,sat1_az,sat1_snr,...*checksum
    fn parse_gpgsv(&self, sentence: &str) -> Option<Vec<SatelliteInfo>> {
        // Vérifier que c'est bien une trame GSV
        if !sentence.starts_with("$GPGSV") && !sentence.starts_with("$GLGSV")
            && !sentence.starts_with("$GAGSV") && !sentence.starts_with("$GBGSV")
            && !sentence.starts_with("$GNGSV") {
            return None;
        }

        debug!("Parsing GPGSV sentence: {}", sentence);

        // Déterminer la constellation
        let constellation = if sentence.starts_with("$GPGSV") {
            "GPS"
        } else if sentence.starts_with("$GLGSV") {
            "GLONASS"
        } else if sentence.starts_with("$GAGSV") {
            "Galileo"
        } else if sentence.starts_with("$GBGSV") {
            "BeiDou"
        } else {
            "GNSS" // Multi-constellation
        };

        let fields: Vec<&str> = sentence.split(',').collect();

        // Minimum 4 champs (header + 3 champs info générale)
        if fields.len() < 4 {
            return None;
        }

        let mut satellites = Vec::new();

        // Parser jusqu'à 4 satellites par trame (champs 4-7, 8-11, 12-15, 16-19)
        for i in 0..4 {
            let base_idx = 4 + (i * 4);

            // Vérifier qu'on a assez de champs
            if base_idx + 3 >= fields.len() {
                break;
            }

            // PRN du satellite
            let prn: u8 = match fields[base_idx].parse() {
                Ok(p) if p > 0 => p,
                _ => continue, // Pas de satellite dans ce slot
            };

            // Élévation (0-90)
            let elevation: u8 = fields[base_idx + 1].parse().unwrap_or(0);

            // Azimut (0-359)
            let azimuth: u16 = fields[base_idx + 2].parse().unwrap_or(0);

            // SNR (peut être vide si pas de signal)
            let snr_field = fields[base_idx + 3].split('*').next().unwrap_or("");
            let snr: u8 = snr_field.parse().unwrap_or(0);

            satellites.push(SatelliteInfo {
                prn,
                elevation,
                azimuth,
                snr,
                constellation: constellation.to_string(),
            });
        }

        if satellites.is_empty() {
            None
        } else {
            Some(satellites)
        }
    }
}

impl Drop for GpsReader {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gprmc() {
        use crate::stats::StatsManager;

        let config = GpsConfig {
            serial_port: "COM9".to_string(),
            baud_rate: 9600,
            sync_timeout: 30,
            min_satellites: 4,
            pps_enabled: true,
            pps_gpio_pin: None,
        };

        let clock = Arc::new(GpsNmeaClock::new(30));
        let stats_manager = StatsManager::new();
        let reader = GpsReader::new(config, clock, stats_manager.clone_arc());

        // Trame GPRMC valide
        let sentence = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let result = reader.parse_gprmc(sentence);

        assert!(result.is_some());
        let (timestamp, _satellites) = result.unwrap();
        // Vérifier que le timestamp est dans une plage raisonnable
        assert!(timestamp.seconds() > 0);
    }

    #[test]
    fn test_parse_gpgga_satellites() {
        use crate::stats::StatsManager;

        let config = GpsConfig {
            serial_port: "COM9".to_string(),
            baud_rate: 9600,
            sync_timeout: 30,
            min_satellites: 4,
            pps_enabled: true,
            pps_gpio_pin: None,
        };

        let clock = Arc::new(GpsNmeaClock::new(30));
        let stats_manager = StatsManager::new();
        let reader = GpsReader::new(config, clock, stats_manager.clone_arc());

        // Trame GPGGA avec 8 satellites
        let sentence = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        let result = reader.parse_gpgga_satellites(sentence);

        assert_eq!(result, Some(8));
    }
}
