use crate::packet::NtpTimestamp;
use std::time::{SystemTime, UNIX_EPOCH};

/// Différence entre l'epoch NTP (1900-01-01) et l'epoch Unix (1970-01-01) en secondes
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// Trait pour les sources d'horloge
pub trait ClockSource: Send + Sync {
    /// Retourne le temps actuel sous forme de timestamp NTP
    fn now(&self) -> NtpTimestamp;

    /// Retourne le type de source d'horloge (pour reference_identifier)
    fn reference_id(&self) -> [u8; 4];

    /// Retourne le stratum (0 pour non synchronisé, 1 pour source primaire)
    fn stratum(&self) -> u8;

    /// Retourne la précision estimée en log2 secondes (ex: -20 = ~1µs)
    fn precision(&self) -> i8;
}

/// Horloge système haute précision
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        SystemClock
    }

    /// Obtient le temps avec la meilleure précision disponible sur la plateforme
    #[cfg(target_os = "windows")]
    fn get_precise_time() -> (u64, u32) {
        // Sur Windows, utiliser GetSystemTimePreciseAsFileTime via SystemTime
        // SystemTime::now() utilise déjà cette API sur Windows 8+
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time before UNIX epoch");

        let unix_seconds = duration.as_secs();
        let nanos = duration.subsec_nanos();

        // Convertir en temps NTP (depuis 1900)
        let ntp_seconds = unix_seconds + NTP_UNIX_OFFSET;

        (ntp_seconds, nanos)
    }

    #[cfg(target_os = "linux")]
    fn get_precise_time() -> (u64, u32) {
        use libc::{clock_gettime, timespec, CLOCK_REALTIME};
        use std::mem::MaybeUninit;

        unsafe {
            let mut ts = MaybeUninit::<timespec>::uninit();
            if clock_gettime(CLOCK_REALTIME, ts.as_mut_ptr()) == 0 {
                let ts = ts.assume_init();
                let unix_seconds = ts.tv_sec as u64;
                let nanos = ts.tv_nsec as u32;

                // Convertir en temps NTP
                let ntp_seconds = unix_seconds + NTP_UNIX_OFFSET;
                (ntp_seconds, nanos)
            } else {
                // Fallback vers SystemTime
                Self::fallback_time()
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn get_precise_time() -> (u64, u32) {
        use libc::{clock_gettime, timespec, CLOCK_REALTIME};
        use std::mem::MaybeUninit;

        unsafe {
            let mut ts = MaybeUninit::<timespec>::uninit();
            if clock_gettime(CLOCK_REALTIME, ts.as_mut_ptr()) == 0 {
                let ts = ts.assume_init();
                let unix_seconds = ts.tv_sec as u64;
                let nanos = ts.tv_nsec as u32;

                let ntp_seconds = unix_seconds + NTP_UNIX_OFFSET;
                (ntp_seconds, nanos)
            } else {
                Self::fallback_time()
            }
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    fn get_precise_time() -> (u64, u32) {
        Self::fallback_time()
    }

    #[allow(dead_code)]
    fn fallback_time() -> (u64, u32) {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time before UNIX epoch");

        let unix_seconds = duration.as_secs();
        let nanos = duration.subsec_nanos();
        let ntp_seconds = unix_seconds + NTP_UNIX_OFFSET;

        (ntp_seconds, nanos)
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockSource for SystemClock {
    fn now(&self) -> NtpTimestamp {
        let (seconds, nanos) = Self::get_precise_time();
        NtpTimestamp::from_seconds_and_nanos(seconds, nanos)
    }

    fn reference_id(&self) -> [u8; 4] {
        // "LOCL" pour horloge locale non synchronisée
        *b"LOCL"
    }

    fn stratum(&self) -> u8 {
        // Stratum 16 = non synchronisé (horloge locale seulement)
        16
    }

    fn precision(&self) -> i8 {
        // Précision typique d'horloge système: ~100ns = 2^-23
        #[cfg(target_os = "windows")]
        return -23; // ~119ns

        #[cfg(target_os = "linux")]
        return -24; // ~60ns avec CLOCK_REALTIME

        #[cfg(target_os = "macos")]
        return -24;

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return -20; // ~1µs par défaut
    }
}

/// Horloge synchronisée avec GPS/GNSS
/// Cette structure sera utilisée quand un module GPS est connecté
pub struct GpsNmeaClock {
    /// Dernière synchronisation GPS reçue
    last_sync: std::sync::Arc<std::sync::RwLock<Option<GpsSync>>>,

    /// Offset PPS : différence entre horloge système et temps GPS réel
    /// Calculé via le signal PPS pour une précision < 1ms
    /// Positif si l'horloge système est en avance sur GPS
    pps_offset: std::sync::Arc<std::sync::RwLock<Option<PpsOffset>>>,

    /// Horloge système comme fallback
    system_clock: SystemClock,

    /// Timeout après lequel on considère la sync GPS périmée (secondes)
    sync_timeout: u64,
}

#[derive(Clone)]
struct GpsSync {
    /// Timestamp de la dernière sync GPS (depuis NMEA)
    timestamp: NtpTimestamp,

    /// Moment système auquel cette sync a été reçue
    system_time: std::time::Instant,

    /// Qualité du signal GPS (nombre de satellites)
    quality: u8,
}

#[derive(Clone)]
struct PpsOffset {
    /// Offset en secondes entre horloge système et temps GPS
    /// offset = système - GPS
    offset_seconds: f64,

    /// Instant système du dernier calcul d'offset
    measured_at: std::time::Instant,

    /// Nombre de mesures PPS utilisées pour calculer cet offset
    sample_count: u32,
}

impl GpsNmeaClock {
    pub fn new(sync_timeout_secs: u64) -> Self {
        GpsNmeaClock {
            last_sync: std::sync::Arc::new(std::sync::RwLock::new(None)),
            pps_offset: std::sync::Arc::new(std::sync::RwLock::new(None)),
            system_clock: SystemClock::new(),
            sync_timeout: sync_timeout_secs,
        }
    }

    /// Met à jour la synchronisation GPS
    /// Cette méthode sera appelée depuis le thread qui lit le port série GPS
    pub fn update_gps_time(&self, gps_timestamp: NtpTimestamp, satellite_count: u8) {
        let sync = GpsSync {
            timestamp: gps_timestamp,
            system_time: std::time::Instant::now(),
            quality: satellite_count,
        };

        if let Ok(mut guard) = self.last_sync.write() {
            *guard = Some(sync);
        }
    }

    /// Met à jour l'offset PPS système-GPS
    /// Appelé quand on détecte un pulse PPS qui correspond au début d'une seconde GPS
    ///
    /// # Arguments
    /// * `pps_instant` - Instant système du pulse PPS
    /// * `gps_second_boundary` - Timestamp GPS de la seconde entière (ex: 11:29:24.000000)
    pub fn update_pps_offset(&self, pps_instant: std::time::Instant, gps_second_boundary: NtpTimestamp) {
        // Convertir l'instant système en timestamp NTP pour comparaison
        let system_ntp = self.system_clock.now();

        // L'instant PPS correspond exactement au début d'une seconde GPS
        // Calculer combien de temps s'est écoulé depuis le PPS
        let elapsed_since_pps = pps_instant.elapsed();

        // Timestamp système au moment du PPS (en reculant dans le temps)
        let system_at_pps_secs = system_ntp.seconds() as f64 - elapsed_since_pps.as_secs_f64();
        let gps_at_pps_secs = gps_second_boundary.seconds() as f64;

        // Offset = système - GPS (positif si système en avance)
        let offset = system_at_pps_secs - gps_at_pps_secs;

        if let Ok(mut guard) = self.pps_offset.write() {
            if let Some(existing) = guard.as_mut() {
                // Filtrage EWMA (Exponentially Weighted Moving Average) pour stabilité
                // 90% ancien + 10% nouveau
                existing.offset_seconds = existing.offset_seconds * 0.9 + offset * 0.1;
                existing.measured_at = std::time::Instant::now();
                existing.sample_count += 1;
            } else {
                // Première mesure
                *guard = Some(PpsOffset {
                    offset_seconds: offset,
                    measured_at: std::time::Instant::now(),
                    sample_count: 1,
                });
            }
        }
    }

    /// Retourne l'offset PPS actuel si disponible
    pub fn get_pps_offset(&self) -> Option<f64> {
        if let Ok(guard) = self.pps_offset.read() {
            guard.as_ref().map(|offset| offset.offset_seconds)
        } else {
            None
        }
    }

    /// Vérifie si la synchronisation GPS est valide
    fn is_gps_synced(&self) -> bool {
        if let Ok(guard) = self.last_sync.read() {
            if let Some(sync) = guard.as_ref() {
                let elapsed = sync.system_time.elapsed().as_secs();
                return elapsed < self.sync_timeout && sync.quality >= 3;
            }
        }
        false
    }

    /// Calcule le temps GPS actuel avec correction PPS
    ///
    /// Méthode professionnelle en 3 étapes :
    /// 1. Si offset PPS disponible : temps_gps = horloge_système - offset_pps (< 1ms précision)
    /// 2. Sinon : extrapoler depuis dernière trame NMEA (précision ~100ms)
    /// 3. Sinon : fallback horloge système
    fn calculate_gps_time(&self) -> Option<NtpTimestamp> {
        // MÉTHODE 1 (préférée) : Utiliser l'offset PPS pour précision maximale
        if let Ok(pps_guard) = self.pps_offset.read() {
            if let Some(pps) = pps_guard.as_ref() {
                // Vérifier que l'offset PPS est récent (< 5 secondes)
                if pps.measured_at.elapsed().as_secs() < 5 {
                    // Obtenir le temps système actuel
                    let system_now = self.system_clock.now();

                    // Extraire les secondes et la fraction correctement
                    let system_secs = system_now.seconds() as f64;
                    // Extraire uniquement les 32 bits bas (fraction)
                    let system_frac_u32 = (system_now.0 & 0xFFFFFFFF) as u32;
                    let system_frac = system_frac_u32 as f64 / (1u64 << 32) as f64;
                    let system_time = system_secs + system_frac;

                    // Appliquer la correction PPS : GPS = système - offset
                    let gps_time = system_time - pps.offset_seconds;

                    // Convertir en NtpTimestamp
                    let gps_secs = gps_time.floor() as u64;
                    let gps_frac = (gps_time.fract() * 1_000_000_000.0) as u32;

                    return Some(NtpTimestamp::from_seconds_and_nanos(gps_secs, gps_frac));
                }
            }
        }

        // MÉTHODE 2 (fallback) : Extrapoler depuis dernière trame NMEA
        if let Ok(guard) = self.last_sync.read() {
            if let Some(sync) = guard.as_ref() {
                let elapsed = sync.system_time.elapsed();

                // Temps GPS + temps écoulé depuis la sync
                let elapsed_secs = elapsed.as_secs();
                let elapsed_nanos = elapsed.subsec_nanos();

                let total_secs = sync.timestamp.seconds() as u64 + elapsed_secs;
                let total_nanos = elapsed_nanos;

                return Some(NtpTimestamp::from_seconds_and_nanos(
                    total_secs,
                    total_nanos,
                ));
            }
        }

        // MÉTHODE 3 : Aucune sync GPS disponible
        None
    }
}

impl ClockSource for GpsNmeaClock {
    fn now(&self) -> NtpTimestamp {
        // Utiliser GPS si disponible, sinon fallback vers horloge système
        if self.is_gps_synced() {
            if let Some(gps_time) = self.calculate_gps_time() {
                return gps_time;
            }
        }

        // Fallback vers horloge système
        self.system_clock.now()
    }

    fn reference_id(&self) -> [u8; 4] {
        if self.is_gps_synced() {
            *b"GPS\0" // Source GPS
        } else {
            *b"LOCL" // Horloge locale (pas synchronisé)
        }
    }

    fn stratum(&self) -> u8 {
        if self.is_gps_synced() {
            1 // Stratum 1 = source primaire (GPS)
        } else {
            16 // Non synchronisé
        }
    }

    fn precision(&self) -> i8 {
        if self.is_gps_synced() {
            -20 // ~1µs avec GPS
        } else {
            self.system_clock.precision()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_clock() {
        let clock = SystemClock::new();
        let ts1 = clock.now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let ts2 = clock.now();

        // Le deuxième timestamp doit être plus grand
        assert!(ts2.seconds() >= ts1.seconds());
    }

    #[test]
    fn test_gps_clock_fallback() {
        let clock = GpsNmeaClock::new(10);

        // Sans sync GPS, doit utiliser horloge système
        assert_eq!(clock.stratum(), 16);
        assert_eq!(&clock.reference_id(), b"LOCL");
    }

    #[test]
    fn test_gps_clock_with_sync() {
        let clock = GpsNmeaClock::new(10);

        // Simuler une sync GPS
        let gps_time = NtpTimestamp::from_seconds_and_nanos(3_900_000_000, 0);
        clock.update_gps_time(gps_time, 8);

        // Doit être en stratum 1
        assert_eq!(clock.stratum(), 1);
        assert_eq!(&clock.reference_id(), b"GPS\0");
    }
}
