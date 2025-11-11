/*!
# Module GPS/GNSS pour synchronisation NTP Stratum 1

Ce module permet d'interfacer le serveur NTP avec un module GPS/GNSS via le protocole NMEA 0183.

## Matériel supporté

Le module fonctionne avec la plupart des modules GPS qui émettent des trames NMEA sur port série :

- **u-blox** : NEO-6M, NEO-7M, NEO-8M, NEO-M9N (recommandé avec PPS)
- **GlobalTop** : PA6H, PA6C
- **Quectel** : L76, L86, L80
- **Adafruit Ultimate GPS** (MTK3339)
- **Raspberry Pi GPS HAT**

## Protocole NMEA

Les trames NMEA utilisées :
- **$GPRMC** : Recommended Minimum data (time, date, position, validity)
- **$GPGGA** : Global Positioning System Fix Data (time, satellites, quality)
- **$GPZDA** : Date & Time (le plus précis pour NTP)

Format typique d'une trame GPRMC :
```text
$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A
       hhmmss   latitude     longitude  speed course date
```

## Connexion série

### Sur Raspberry Pi / Linux
```bash
# Port série : /dev/ttyUSB0, /dev/ttyAMA0, /dev/serial0
# Baud rate : généralement 9600 (configurable)

# Vérifier la présence du GPS
ls -la /dev/ttyUSB* /dev/ttyAMA* /dev/serial*

# Tester la réception NMEA
cat /dev/ttyUSB0
```

### Sur Windows
```
Port série : COM3, COM4, etc.
Vérifier dans "Gestionnaire de périphériques" > "Ports (COM et LPT)"
```

## PPS (Pulse Per Second)

Le signal PPS est crucial pour la précision d'un serveur NTP Stratum 1.
Il fournit une impulsion électrique extrêmement précise chaque seconde.

### Avantages du PPS
- Précision : < 1 microseconde
- Indépendant des délais série NMEA
- Signal matériel direct

### Configuration PPS sur Linux

1. **Activer le kernel module**
```bash
# Charger le module pps-gpio
sudo modprobe pps-gpio

# Vérifier
lsmod | grep pps
```

2. **Configuration Raspberry Pi** (`/boot/config.txt`)
```ini
# Activer PPS sur GPIO 18
dtoverlay=pps-gpio,gpiopin=18
```

3. **Vérifier le signal PPS**
```bash
# Afficher les devices PPS
ls /dev/pps*

# Tester la réception PPS
sudo ppstest /dev/pps0
```

Sortie attendue :
```
trying PPS source "/dev/pps0"
found PPS source "/dev/pps0"
ok, found 1 source(s), now start fetching data...
source 0 - assert 1699876543.000000123, sequence: 12345
```

### Câblage PPS

Module GPS → Raspberry Pi :
- PPS pin → GPIO 18 (ou autre GPIO configuré)
- GND → GND
- VCC → 3.3V ou 5V
- TX → RX (GPIO 15)
- RX → TX (GPIO 14)

## Précision attendue

| Configuration          | Précision typique |
|------------------------|-------------------|
| NMEA seul              | ~10-100 ms        |
| NMEA + PPS (kernel)    | < 1 µs            |
| NMEA + PPS + chrony    | < 100 ns          |

## Intégration avec ce serveur NTP

Le module `GpsNmeaClock` dans `clock.rs` gère :
1. Lecture du port série GPS
2. Parsing des trames NMEA (GPRMC, GPGGA, GPZDA)
3. Validation de la qualité du signal (nombre de satellites)
4. Synchronisation du timestamp NTP

Pour activer :
```toml
[clock]
source = "gps"

[clock.gps]
serial_port = "/dev/ttyUSB0"
baud_rate = 9600
sync_timeout = 30
min_satellites = 4
pps_enabled = true
pps_gpio_pin = 18
```

## TODO : Implémentation complète

Ce module est actuellement une documentation. L'implémentation complète nécessiterait :

1. **Crate serialport** : pour la communication série
```toml
serialport = "4.2"
```

2. **Parser NMEA** :
   - Crate `nmea` ou parser custom
   - Extraction de l'heure UTC des trames
   - Validation checksum

3. **Thread de lecture GPS**
   - Lecture asynchrone du port série
   - Parsing des trames
   - Mise à jour de `GpsNmeaClock`

4. **Support PPS (optionnel mais recommandé)**
   - Lecture de `/dev/pps0` via ioctl
   - Discipline de l'horloge système avec PPS
   - Crate `nix` pour les appels système Linux

## Exemple d'utilisation future

```rust
// Dans main.rs
let gps_config = config.clock.gps.unwrap();
let gps_clock = Arc::new(GpsNmeaClock::new(gps_config.sync_timeout));

// Démarrer le thread GPS
let gps_clock_clone = Arc::clone(&gps_clock);
std::thread::spawn(move || {
    let mut port = serialport::new(&gps_config.serial_port, gps_config.baud_rate)
        .open()
        .expect("Failed to open GPS serial port");

    let mut reader = std::io::BufReader::new(port);
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).is_ok() {
            if let Some(timestamp) = parse_nmea_time(&line) {
                gps_clock_clone.update_gps_time(timestamp, 8);
            }
        }
    }
});

// Créer le serveur avec l'horloge GPS
let server = NtpServer::new(config, gps_clock);
server.run()?;
```

## Alternatives professionnelles

Pour un déploiement en production, considérer :

1. **chrony** : Daemon NTP professionnel avec support PPS natif
   - Meilleure discipline d'horloge
   - Gestion automatique des dérives
   - Support PPS kernel intégré

2. **ntpsec** : Fork sécurisé de NTP avec support GPS

3. **Serveur dédié** :
   - Stratum 1 NTP appliance commerciale
   - Oscillateur TCXO/OCXO pour stabilité
   - Redondance GPS multi-constellation (GPS + GLONASS + Galileo)

## Ressources

- [RFC 5905](https://tools.ietf.org/html/rfc5905) : Network Time Protocol Version 4
- [NMEA 0183 Specification](http://www.nmea.org/)
- [Linux PPS Documentation](https://www.kernel.org/doc/html/latest/driver-api/pps.html)
- [Raspberry Pi PPS Setup](https://www.satsignal.eu/ntp/Raspberry-Pi-NTP.html)
*/

// Note: Le parsing NMEA réel est implémenté dans src/gps_reader.rs
// Ce fichier contient uniquement la documentation
/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nmea_rmc() {
        let sentence = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
        let result = parse_nmea_rmc(sentence);

        assert!(result.is_some());
        let (year, month, day, seconds) = result.unwrap();
        assert_eq!(year, 2094);
        assert_eq!(month, 3);
        assert_eq!(day, 23);
        assert_eq!(seconds, 12 * 3600 + 35 * 60 + 19);
    }

    #[test]
    fn test_parse_nmea_gga_satellites() {
        let sentence = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
        let result = parse_nmea_gga_satellites(sentence);

        assert_eq!(result, Some(8));
    }

    #[test]
    fn test_leap_year() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
    }
}
*/