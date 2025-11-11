use thiserror::Error;

/// Erreurs liées au parsing des paquets NTP
#[derive(Error, Debug)]
pub enum NtpError {
    #[error("Invalid packet size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("Invalid NTP version: {0}")]
    InvalidVersion(u8),

    #[error("Invalid NTP mode: {0}")]
    InvalidMode(u8),

    #[allow(dead_code)]
    #[error("Stratum out of range: {0}")]
    InvalidStratum(u8),
}

/// Leap Indicator values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapIndicator {
    NoWarning = 0,
    LastMinute61Seconds = 1,
    LastMinute59Seconds = 2,
    AlarmCondition = 3,
}

impl From<u8> for LeapIndicator {
    fn from(value: u8) -> Self {
        match value & 0b11 {
            0 => LeapIndicator::NoWarning,
            1 => LeapIndicator::LastMinute61Seconds,
            2 => LeapIndicator::LastMinute59Seconds,
            _ => LeapIndicator::AlarmCondition,
        }
    }
}

/// NTP Mode values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtpMode {
    Reserved = 0,
    SymmetricActive = 1,
    SymmetricPassive = 2,
    Client = 3,
    Server = 4,
    Broadcast = 5,
    NtpControlMessage = 6,
    ReservedPrivate = 7,
}

impl NtpMode {
    pub fn from_u8(value: u8) -> Result<Self, NtpError> {
        match value & 0x07 {
            0 => Ok(NtpMode::Reserved),
            1 => Ok(NtpMode::SymmetricActive),
            2 => Ok(NtpMode::SymmetricPassive),
            3 => Ok(NtpMode::Client),
            4 => Ok(NtpMode::Server),
            5 => Ok(NtpMode::Broadcast),
            6 => Ok(NtpMode::NtpControlMessage),
            7 => Ok(NtpMode::ReservedPrivate),
            _ => Err(NtpError::InvalidMode(value)),
        }
    }
}

/// Structure représentant un timestamp NTP (64 bits)
/// Format: 32 bits de secondes + 32 bits de fraction
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NtpTimestamp(pub u64);

impl NtpTimestamp {
    /// Crée un timestamp NTP à partir de secondes et nanosecondes depuis l'epoch NTP (1900-01-01)
    pub fn from_seconds_and_nanos(seconds: u64, nanos: u32) -> Self {
        let fraction = ((nanos as u64) << 32) / 1_000_000_000;
        NtpTimestamp((seconds << 32) | fraction)
    }

    /// Convertit le timestamp en format big-endian pour transmission réseau
    #[allow(dead_code)]
    pub fn to_be(self) -> u64 {
        self.0.to_be()
    }

    /// Crée un timestamp depuis un format big-endian reçu du réseau
    #[allow(dead_code)]
    pub fn from_be(value: u64) -> Self {
        NtpTimestamp(u64::from_be(value))
    }

    /// Retourne la partie secondes du timestamp
    pub fn seconds(&self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// Retourne la partie fraction du timestamp
    pub fn fraction(&self) -> u32 {
        self.0 as u32
    }
}

/// Structure du paquet NTP (48 octets)
/// Tous les champs multi-octets sont en big-endian (network byte order)
#[derive(Debug, Clone, Copy)]
pub struct NtpPacket {
    // Octet 0
    pub leap_indicator: LeapIndicator,
    pub version: u8,
    pub mode: NtpMode,

    // Octet 1-3
    pub stratum: u8,
    pub poll: i8,
    pub precision: i8,

    // Octets 4-7
    pub root_delay: u32,

    // Octets 8-11
    pub root_dispersion: u32,

    // Octets 12-15
    pub reference_identifier: u32,

    // Octets 16-23
    pub reference_timestamp: NtpTimestamp,

    // Octets 24-31
    pub originate_timestamp: NtpTimestamp,

    // Octets 32-39
    pub receive_timestamp: NtpTimestamp,

    // Octets 40-47
    pub transmit_timestamp: NtpTimestamp,
}

impl NtpPacket {
    /// Taille du paquet NTP en octets
    pub const SIZE: usize = 48;

    /// Crée un paquet de réponse serveur par défaut
    pub fn new_server_response() -> Self {
        NtpPacket {
            leap_indicator: LeapIndicator::NoWarning,
            version: 4,
            mode: NtpMode::Server,
            stratum: 1, // Stratum 1 = source primaire (GPS/GNSS)
            poll: 4,
            precision: -20, // 2^-20 ≈ 1 microseconde
            root_delay: 0,
            root_dispersion: 0,
            reference_identifier: u32::from_be_bytes(*b"GPS\0"), // "GPS" pour source GPS
            reference_timestamp: NtpTimestamp::default(),
            originate_timestamp: NtpTimestamp::default(),
            receive_timestamp: NtpTimestamp::default(),
            transmit_timestamp: NtpTimestamp::default(),
        }
    }

    /// Parse un buffer en paquet NTP
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NtpError> {
        if bytes.len() < Self::SIZE {
            return Err(NtpError::InvalidSize {
                expected: Self::SIZE,
                actual: bytes.len(),
            });
        }

        // Octet 0: LI (2 bits) + VN (3 bits) + Mode (3 bits)
        let li_vn_mode = bytes[0];
        let leap_indicator = LeapIndicator::from((li_vn_mode >> 6) & 0x03);
        let version = (li_vn_mode >> 3) & 0x07;
        let mode = NtpMode::from_u8(li_vn_mode & 0x07)?;

        // Validation de la version (accepter NTPv1 à v4 pour compatibilité)
        if version < 1 || version > 4 {
            return Err(NtpError::InvalidVersion(version));
        }

        // Octet 1-3
        let stratum = bytes[1];
        let poll = bytes[2] as i8;
        let precision = bytes[3] as i8;

        // Octets 4-47: Tous en big-endian
        let root_delay = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let root_dispersion = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let reference_identifier = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);

        let reference_timestamp = NtpTimestamp(u64::from_be_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19],
            bytes[20], bytes[21], bytes[22], bytes[23],
        ]));

        let originate_timestamp = NtpTimestamp(u64::from_be_bytes([
            bytes[24], bytes[25], bytes[26], bytes[27],
            bytes[28], bytes[29], bytes[30], bytes[31],
        ]));

        let receive_timestamp = NtpTimestamp(u64::from_be_bytes([
            bytes[32], bytes[33], bytes[34], bytes[35],
            bytes[36], bytes[37], bytes[38], bytes[39],
        ]));

        let transmit_timestamp = NtpTimestamp(u64::from_be_bytes([
            bytes[40], bytes[41], bytes[42], bytes[43],
            bytes[44], bytes[45], bytes[46], bytes[47],
        ]));

        Ok(NtpPacket {
            leap_indicator,
            version,
            mode,
            stratum,
            poll,
            precision,
            root_delay,
            root_dispersion,
            reference_identifier,
            reference_timestamp,
            originate_timestamp,
            receive_timestamp,
            transmit_timestamp,
        })
    }

    /// Convertit le paquet en bytes pour transmission (big-endian)
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];

        // Octet 0: LI + VN + Mode
        bytes[0] = ((self.leap_indicator as u8) << 6)
                 | ((self.version & 0x07) << 3)
                 | (self.mode as u8 & 0x07);

        // Octets 1-3
        bytes[1] = self.stratum;
        bytes[2] = self.poll as u8;
        bytes[3] = self.precision as u8;

        // Octets 4-7: Root delay (big-endian)
        bytes[4..8].copy_from_slice(&self.root_delay.to_be_bytes());

        // Octets 8-11: Root dispersion (big-endian)
        bytes[8..12].copy_from_slice(&self.root_dispersion.to_be_bytes());

        // Octets 12-15: Reference identifier (big-endian)
        bytes[12..16].copy_from_slice(&self.reference_identifier.to_be_bytes());

        // Octets 16-23: Reference timestamp (big-endian)
        bytes[16..24].copy_from_slice(&self.reference_timestamp.0.to_be_bytes());

        // Octets 24-31: Originate timestamp (big-endian)
        bytes[24..32].copy_from_slice(&self.originate_timestamp.0.to_be_bytes());

        // Octets 32-39: Receive timestamp (big-endian)
        bytes[32..40].copy_from_slice(&self.receive_timestamp.0.to_be_bytes());

        // Octets 40-47: Transmit timestamp (big-endian)
        bytes[40..48].copy_from_slice(&self.transmit_timestamp.0.to_be_bytes());

        bytes
    }

    /// Valide qu'il s'agit d'une requête client valide
    #[allow(dead_code)]
    pub fn is_valid_client_request(&self) -> bool {
        self.mode == NtpMode::Client
            && self.version >= 3
            && self.version <= 4
            && self.transmit_timestamp.0 != 0 // Le client doit avoir mis un timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ntp_timestamp_conversion() {
        let ts = NtpTimestamp::from_seconds_and_nanos(3_900_000_000, 500_000_000);
        assert_eq!(ts.seconds(), 3_900_000_000);

        // Test round-trip big-endian
        let be = ts.to_be();
        let ts2 = NtpTimestamp::from_be(be);
        assert_eq!(ts, ts2);
    }

    #[test]
    fn test_packet_serialization() {
        let packet = NtpPacket::new_server_response();
        let bytes = packet.to_bytes();
        let parsed = NtpPacket::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.version, 4);
        assert_eq!(parsed.mode, NtpMode::Server);
        assert_eq!(parsed.stratum, 1);
    }
}
