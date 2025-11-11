# Journal des modifications

Tous les changements notables de ce projet seront documentés dans ce fichier.

Le format est basé sur [Keep a Changelog](https://keepachangelog.com/fr/1.0.0/),
et ce projet adhère au [Semantic Versioning](https://semver.org/lang/fr/).

## [0.1.0] - 2024-11-11

### Ajouté

#### Serveur NTP
- Implémentation complète du protocole NTP v4 conforme à la RFC 5905
- Support Stratum 1 avec synchronisation GPS
- Serveur UDP sur port 123 avec traitement asynchrone
- Timestamps haute précision (receive et transmit distincts)

#### Module GPS
- Lecteur GPS avec support NMEA standard (trames GPRMC et GPGGA)
- Thread GPS séparé avec reconnexion automatique
- Exponential backoff pour les tentatives de reconnexion (5s à 60s)
- Détection du signal PPS via ligne CTS du port série
- Parsing automatique du nombre de satellites et de la qualité du signal
- Timeout configurable avec fallback vers horloge système
- Support multi-plateforme (Windows COM, Linux /dev/tty)
- Compatible avec modules u-blox NEO-7M, NEO-M9N, Adafruit Ultimate GPS (liste non-exhaustive)

#### Interface web
- Dashboard de monitoring en temps réel sur port 8080
- Horloge précise avec affichage date, heure et millisecondes
- Indicateurs visuels :
  - Statut GPS (vert = synchronisé, gris = non synchronisé)
  - Signal PPS (clignote à chaque seconde)
  - Activité USB RX/TX (clignote lors de transmission)
- Statistiques NTP en temps réel :
  - Requêtes reçues, traitées, rejetées
  - Nombre d'erreurs
- Informations système :
  - Stratum actuel
  - Précision de l'horloge
  - Reference ID (GPS ou LOCL)
  - Uptime du serveur
- Mise à jour automatique via WebSocket sans rechargement
- Design moderne avec dégradés et effets de transparence

#### Sécurité
- Rate limiting par adresse IP
- Limitation du nombre de requêtes par seconde (configurable)
- Validation stricte des paquets NTP (mode client, version)
- Support liste blanche et liste noire d'adresses IP
- Protection contre les attaques par amplification
- Validation de la longueur des paquets

#### Configuration
- Fichier de configuration TOML avec valeurs par défaut intelligentes
- Génération automatique du fichier config.toml commenté au premier lancement

#### Logs et observabilité
- Logs structurés avec la bibliothèque tracing
- Niveaux de log configurables (trace, debug, info, warn, error)
- Support de la variable d'environnement RUST_LOG (RUST_LOG=DEBUG par exemple)
- Logs dans fichier et console
- Statistiques périodiques (requêtes, erreurs)
- Logs détaillés du GPS (connexion, synchronisation, PPS)

#### Robustesse
- Aucun panic possible (toutes les erreurs gérées avec Result)
- Gestion d'erreur complète pour toutes les opérations I/O
- Le serveur NTP continue de fonctionner même sans GPS
- Transition transparente entre GPS et horloge système
- Ajustement automatique du Stratum selon la source d'horloge

### Notes techniques

#### Précision attendue
- Horloge système seule : 10-100 µs, Stratum 16
- GPS sans PPS : 10-50 ms, Stratum 1
- GPS + PPS via CTS : < 1 ms, Stratum 1
- GPS + PPS kernel Linux : < 10 µs, Stratum 1

#### Modules Rust utilisés
- axum : Serveur web et WebSocket
- tokio : Runtime asynchrone
- serde/toml : Configuration
- tracing : Logs structurés
- serialport : Communication série GPS
- chrono : Gestion des dates/heures
- anyhow/thiserror : Gestion d'erreurs
- libc : Accès horloge système haute précision

## Notes de version

### À propos de la version 0.1.0

Cette première version publique de Pendulum représente un serveur NTP Stratum 1 entièrement fonctionnel et prêt pour la production. Le projet a été développé avec un accent particulier sur :

1. **Correction technique** : Les problèmes d'endianness et de timestamps identiques qui affectaient les premières versions ont été complètement résolus, permettant une précision de l'ordre de la microseconde.

2. **Robustesse** : Le serveur est conçu pour ne jamais crasher, avec gestion complète des erreurs et fallback intelligent en cas de défaillance GPS.

3. **Expérience utilisateur** : Configuration automatique, interface web moderne, et documentation complète en français.

4. **Précision** : Avec un petit module GPS et un convertisseur FTDI-232, faciles à trouver, le serveur atteint une précision inférieure à la milliseconde, comparable aux solutions commerciales d'entrée de gamme.

Le projet est particulièrement adapté pour :
- Laboratoires et universités nécessitant une source de temps locale précise
- Réseaux isolés sans accès Internet
- Projets DIY de passionnés d'électronique et de précision temporelle
- Apprentissage du protocole NTP et de la synchronisation GPS
- Serveurs de temps pour petites organisations

### Compatibilité

**Plateformes testées** :
- Windows 10/11 (x86_64)
- Linux (x86_64, ARM64)
- Raspberry Pi 4 (Raspberry Pi OS)

**Module GPS testé** :
- KeyeStudio / u-blox NEO-7M 

**Convertisseur USB-série testé** :
- FTDI FT232RL


Le projet est sous licence MIT et accueille toutes les contributions constructives.

---

[0.1.0]: https://github.com/Le-Stef/pendulum/releases/tag/v0.1.0
