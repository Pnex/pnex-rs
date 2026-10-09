# Profils de sécurité : du maker au militaire (roadmap)

> **Statut : roadmap tracée le 2026-10-08 (D148–D152), rien n'est
> implémenté.** Ce document fixe la cible finale (exigences les plus
> élevées, jusqu'au militaire) et le mécanisme qui les rendra activables
> client par client. L'implémentation viendra par vagues (§6).
>
> **Changement de direction du 2026-10-08 (D153–D157)** : avant la
> release, le protocole device bascule **directement** sur le modèle
> industriel (X.509 + TLS mutuel, AEAD interne standard), sans étape
> maison intermédiaire ni compatibilité avec les devices déjà flashés.
> Cela remonte EX-A4, A5 et A6 en V1 (§5.A, §6).
>
> **Gel matériel** : aucun eFuse n'est brûlé sur aucune carte tant que le
> firmware n'a pas été validé 1 à 2 ans par la communauté (§6, V4).
> D'ici là, seules les mesures logicielles et réversibles sont
> implémentables.

## 1. Décisions

| # | Décision |
|---|---|
| D148 | **Profils de sécurité gradués activés par flags** : `open` (défaut, communauté), `industrial`, `critical`, `sovereign`. Un même produit sert les makers et les clients les plus exigeants ; le profil `open` reste le défaut, pour toujours. |
| D149 | **Rien d'irréversible par défaut.** Tout eFuse exige : un profil explicite, une carte compatible, la preuve que les clés sont sauvegardées, une confirmation humaine. Jamais sur une carte du banc HIL. Gel total jusqu'à la fin de la phase communautaire (V4). |
| D150 | **PneX ne détient jamais seul les clés d'un client durci.** Profils `critical` et `sovereign` : clés du client (sa PKI, son HSM). Profil `industrial` : clés gardées par le serveur du client (self-hosted) ou exportables. Profil `open` : clés serveur, sans eFuse, donc sans enjeu de verrouillage. |
| D151 | **Le matériel reste réutilisable par son propriétaire.** Durcissement avec clés générées hors puce et conservées, Secure Download Mode (jamais le download désactivé, sauf `sovereign`), clé Secure Boot de secours. Anti-downgrade **logiciel** (NVS) en `open`/`industrial` ; l'eFuse `secure_version` seulement en `critical`+ . |
| D152 | **Les exigences sont tracées maintenant, implémentées par vagues** (§6). Les trous de protocole du profil `open` (SEC-17 à SEC-19) sont corrigés dans la première vague, **pour tous les profils**. |
| D153 | **Identité device = X.509 + TLS mutuel, dès V1 et pour tous les profils** (modèle AWS IoT Core), en remplacement du jeton. Une CA par org (prépare la PKI du client, D150 / EX-A7) ; certificat et clé émis au build (le firmware est déjà compilé par device). nginx (D70) vérifie le certificat client sur `/ws/` (`ssl_verify_client optional`) et transmet l'empreinte vérifiée au backend dans un en-tête qu'il efface s'il vient du client ; le backend associe empreinte → device. Révocation = registre (pas de CRL). Rauthy n'intervient pas : c'est une PKI, pas un fournisseur d'identité ; Rauthy reste l'IdP des humains (D19). |
| D154 | **TLS obligatoire** : `ws://` refusé, jeton (repli) hors de l'URL. |
| D155 | **La couche interne chiffrée est conservée sous TLS, et ce n'est pas une redondance.** TLS échange ses clés en ECDHE, cassable par un ordinateur quantique (« harvest now, decrypt later ») ; une clé partagée de 256 bits ne l'est pas (Grover → 128 bits effectifs). TLS apporte l'identité standard et la confidentialité persistante ; la couche interne apporte la confidentialité post-quantique et l'intégrité de bout en bout. **Ne pas la supprimer en la croyant inutile.** |
| D156 | **Couche interne = Noise, standard crypto de PneX** (choix utilisateur 2026-10-08, remplace la variante HKDF-SHA256 + ChaCha20-Poly1305 notée d'abord) : device ↔ serveur en `Noise_NNpsk0_25519_ChaChaPoly_SHA256` (comme ESPHome), la clé partagée de 256 bits mélangée dès le premier message (protection post-quantique, D155), nonces = compteurs de Noise (anti-rejeu), prologue lié au device. **Le M2M piloté par PneX (M1–M3) réutilise le même framework** avec d'autres motifs Noise (clés statiques) : une seule pile crypto, une seule suite de vecteurs, côté Rust (`snow`) et C++. |
| D157 | **Avant la release, aucune compatibilité ascendante** : le format D8 (ChaCha20 sans Poly1305) est supprimé partout (serveur, agent, firmware), sans indicateur anti-downgrade ni migration. Un device non recompilé est refusé et doit être reflashé. **Repli moins sûr** (wss + jeton, sans certificat client) : profil `open` seulement, et seulement pour une carte qui échoue au banc HIL (inconnue principale : mémoire et durée de la poignée de main ECDSA sur ESP8266). L'edge agent suit le même modèle. |
| D158 | **Point d'entrée devices dédié, jamais derrière un proxy qui termine le TLS** (sinon le certificat client n'arrive pas à nginx). Nom compilé dans le firmware = variable serveur **`PNEX_DEVICE_HOST`** (chart : `deviceHost`), par défaut l'hôte de l'instance (`PNEX_PROD_HOST` ou référentiel Hôtes — indispensable en LAN où l'hôte est une IP nue) ; en cloud `devices.` + hôte (`dev.pnex.io` → `devices.dev.pnex.io`) ; DNS seul (enregistrement `devices.dev` ou `*.dev`, le wildcard `*.pnex.io` ne couvre qu'un niveau). Certificat serveur émis par la **CA PneX de l'instance** (pas Let's Encrypt) : le firmware épingle une CA qui ne change jamais, même modèle en cloud, en self-hosted LAN (D70) et en réseau isolé ; l'épinglage GTS Root R4 / ISRG du chart pnex-deploy devient sans objet pour les devices. Port 443 = WebSocket des devices, **8883 réservé à MQTT** en TLS mutuel (même CA, mêmes certificats). L'interface (`dev.pnex.io`) reste en Let's Encrypt, le site vitrine `pnex.io` peut rester derrière Cloudflare. Règle : ne jamais passer en proxy un enregistrement qui résout l'hôte devices. |

## 2. Pourquoi des profils

L'adoption passe par la communauté (makers, écoles, fablabs) : leurs
cartes doivent rester libres, reflashables par n'importe qui. Les
industriels veulent l'inverse : des cartes verrouillées, mais **par
eux**. Les deux ne sont compatibles que si :

1. la sécurité par défaut est invisible et réversible (profil `open`) ;
2. tout verrouillage est un choix explicite, par org et par device ;
3. PneX n'est jamais le seul détenteur des clés d'un client.

Argument produit, à reprendre tel quel : *« vos cartes restent les
vôtres : reflashables à vie, et pourtant trafic et mises à jour
authentifiés »*.

## 3. Les profils

| Profil | Public | Référentiels visés | Niveau IEC 62443 | Matériel admis | Irréversible |
|---|---|---|---|---|---|
| `open` | Makers, écoles, communauté, démos | CRA (sécurité par défaut), ETSI EN 303 645 | ≈ SL1 | Toutes cartes (ESP8266 compris) | Non |
| `industrial` | PME/ETI, intégrateurs, bâtiment | IEC 62443-3-3 / 4-2, exigences NIS2 des clients | SL2 | ESP32-C3, S3, C6 pour le durcissement matériel ; ESP32 classique en logiciel seulement | Optionnel, par device |
| `critical` | OIV/OSE : énergie, eau, transport, pharma GxP | IEC 62443 SL3, guides ANSSI (systèmes industriels), FDA 21 CFR Part 11 | SL3 | C3, S3, C6 durcis ; transport filaire recommandé | Oui |
| `sovereign` | Défense, nucléaire | IEC 62443 SL4, homologation ANSSI, instructions sur la protection de l'information (II 901 pour la Diffusion Restreinte) | SL4 | **À définir** (cf. ci-dessous) | Oui |

**Honnêteté sur `sovereign`.** Un ESP32 du commerce ne sera
probablement pas accepté tel quel : pas d'élément sécurisé certifié, et
le sans-fil est souvent interdit sur ces sites. À ce niveau, le rôle
réaliste de PneX est la **supervision sur réseau isolé**, avec des
devices filaires, un élément sécurisé (type ATECC608 ou SE050) ou des
automates certifiés derrière une passerelle. Hypothèse à confirmer avec
un client réel avant tout investissement.

Les cibles SL sont indicatives : la correspondance exacte exigence par
exigence demande le texte de la norme IEC 62443-4-2 (payant), à acquérir
avant la vague V3.

## 4. Mécanisme de flags

Trois niveaux, du plus large au plus fin :

| Niveau | Réglage | Qui le change | Effet |
|---|---|---|---|
| Plateforme | `deployment_mode` : `saas` / `self_hosted` / `airgap` ; `security_floor` : profil minimal imposé à toutes les orgs | Admin plateforme (config) | Un déploiement `critical` refuse toute org en dessous du plancher |
| Org | `security_profile` | Monter : owner. Descendre : admin plateforme seul, tracé au journal d'audit | Défauts et refus de l'org (ex. `ws://` refusé dès `industrial`) |
| Device | Flags de durcissement effectifs | Le build et le provisioning, selon le profil de l'org | Les flags matériels deviennent des **faits** enregistrés (« eFuse Secure Boot brûlé le … »), jamais remis à faux |

Règles :

- **Monotonie** : baisser le profil d'une org ne « dé-durcit » aucune
  carte ; l'UI affiche alors les devices plus durcis que le profil.
- **Refus serveur** : un flag matériel demandé pour une carte
  incompatible est refusé avec un code machine
  (`hardening-board-unsupported`, à créer avec sa clé `err-*` en V4).
- **Matrice de compatibilité** dans le catalogue des cartes
  (`pnex_core::catalog`, D121) : chaque carte déclare les durcissements
  qu'elle supporte. Les cartes HIL sont marquées non durcissables.
- **Chaque flag est testé** : un profil se vérifie par une suite qui
  affirme les refus attendus (même école que les gardes `err_codes`).

## 5. Catalogue des exigences

Colonnes : profil minimal (O = open, I = industrial, C = critical,
S = sovereign ; « opt » = optionnel à ce niveau), impact
(**L** = logiciel réversible, **E** = eFuse irréversible,
**P** = processus/documentation), vague (§6), état actuel.

### A. Protocole device ↔ serveur

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-A1 | Frames chiffrées **authentifiées** : ChaCha20-Poly1305 (AEAD) | O | L | V1 | Fait 2026-10-08 — lien Noise NNpsk0 (D156), lot L1 |
| EX-A2 | **Anti-rejeu** : compteur monotone par session dans les données authentifiées, rejet s'il ne croît pas, des deux côtés | O | L | V1 | Fait 2026-10-08 — lien Noise NNpsk0 (D156), lot L1 |
| EX-A3 | Plus de repli « clé vide = trafic en clair » : `#error` hors build mock | O | L | V1 | Fait 2026-10-08 — lien Noise NNpsk0 (D156), lot L1 |
| EX-A4 | Jeton device hors de l'URL (en-tête ou premier message) — repli D157 seulement | O | L | V1 (D154) | Fait 2026-10-09 (lot L2) — `Authorization: Bearer` sur `/ws/device`, `/ws/sensor/ingest`, `/ws/camera`, téléchargement OTA ; jeton en URL ignoré ; validé sur NodeMCU + C6 |
| EX-A5 | TLS obligatoire : `ws://` refusé | O | L | V1 (D154) | Fait 2026-10-09 (lot L2) — lien device refusé sans `X-Forwarded-Proto: https` de l'edge (close 4013), `ingestion.require_tls` (faux en test seulement) |
| EX-A6 | **Identité par device en X.509** + TLS mutuel, en remplacement du jeton partagé | O (repli jeton en `open`, D157) | L | V1 (D153) | Fait 2026-10-09 (lots L3–L5) — CA ECDSA P-256 par org (clé au coffre), certificat par build, point d'entrée devices dédié (`PNEX_DEVICE_PORT`, 4443 en LAN) en `ssl_verify_client optional_no_ca`, vérification backend registre + chaîne + appartenance au device (close 4014) ; validé sur NodeMCU (BearSSL) et C6 ; agent edge et déploiement (compose, Helm 0.3.0) faits en L6 |
| EX-A7 | Certificats émis par **l'autorité du client** (BYOK, PKCS#11 / HSM) | C | L | V3 | Absent |
| EX-A8 | Rotation des clés et des certificats sans reflash USB | I | L | V3 | Rotation de CA = rebuild + OTA |
| EX-A9 | Crypto agile, algorithmes conformes aux recommandations ANSSI ; module FIPS 140-3 en option | S | L | V5 | Absent |
| EX-A10 | **Confidentialité post-quantique** : couche interne à clé partagée sous TLS (D155) ; limite connue : la clé partagée voyage dans l'image firmware (téléchargement navigateur, OTA) en TLS classique. Pistes : échange hybride `X25519MLKEM768` sur nginx (OpenSSL ≥ 3.5) côté navigateur, OTA chiffrée avec la clé courante | O | L | V1 (couche) / V3 (transport de la clé) | ChaCha20 sans authentification (SEC-17) |
| EX-A11 | TLS hybride classique + post-quantique (ML-KEM) jusqu'au device, recommandation ANSSI | C | L | Quand mbedTLS / ESP-IDF le proposent | Absent : ni BearSSL ni le mbedTLS des cores Arduino |

### B. OTA et cycle de vie du firmware

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-B1 | Image **signée** (Ed25519), clé publique embarquée, vérifiée avant bascule | O | L | V1 | Fait 2026-10-09 (SEC-18, ota.md §4) — banc matériel à faire |
| EX-B2 | Anti-downgrade **logiciel** sur toutes les puces (version minimale en NVS) | O | L | V1 | Fait 2026-10-09 sur toutes les puces, sans NVS : version signée comparée à celle de l'image en cours (ota.md §4) |
| EX-B3 | Fenêtres de maintenance et approbation avant toute OTA | I | L | V2 | OTA immédiate |
| EX-B4 | Déploiement progressif (canari, puis lots), arrêt automatique sur échec | I | L | V2 | OTA en lot manuelle |
| EX-B5 | Approbation à deux personnes (deploy de flow, OTA, changement de secret) | C | L | V3 | Absent |
| EX-B6 | Anti-rollback **eFuse** `secure_version` | C opt, S | E | V5 | Absent |
| EX-B7 | Signature de l'image **hors du builder** (le bac à sable bwrap ne voit jamais les clés) | I | L | V2 | Sans objet tant qu'EX-B1 n'existe pas |
| EX-B8 | Fin de vie d'un device : effacement des secrets, révocation de l'identité, trace | I | L+P | V3 | Suppression simple |

### C. Matériel

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-C1 | Secrets au repos chiffrés : NVS chiffré par HMAC (C3, S3, C6), sans verrouiller le flash | I opt | E (léger) | V4 | Secrets en clair en flash (SEC-20) |
| EX-C2 | **Secure Boot v2** avec clés hors puce + clé de secours | I opt, C | E | V4 | Absent |
| EX-C3 | **Flash Encryption** mode release, clé générée hors puce et conservée | I opt, C | E | V4 | Absent (SEC-20) |
| EX-C4 | Secure Download Mode (reflash USB possible, lecture impossible) | avec EX-C3 | E | V4 | Absent |
| EX-C5 | JTAG désactivé | C | E | V4 | Absent |
| EX-C6 | Download UART totalement désactivé (**plus aucun reflash USB, à vie**) | S seulement | E | V5 | Absent — volontairement réservé |
| EX-C7 | Élément sécurisé pour l'identité (ATECC608, SE050…) | S | matériel | V5 | Absent |
| EX-C8 | Détection d'ouverture du boîtier / effacement des clés | S | matériel | V5 | Hors périmètre actuel |

Prérequis techniques de V4 (à lever avant de brûler quoi que ce soit) :

- le framework Arduino précompilé bloque Secure Boot et Flash
  Encryption : passage à ESP-IDF (Arduino en composant, ou pioarduino)
  ou au firmware Rust ;
- vérifier que le flash navigateur (esptool-js) fonctionne en Secure
  Download Mode (sans stub). Sinon, une carte durcie se reflashe
  seulement avec l'app desktop ;
- vérifier l'écriture d'images pré-chiffrées en mode release sur
  ESP32 classique ;
- mesurer l'impact du chiffrement du flash sur ESP32-CAM (débit
  caméra) et la place des slots OTA avec le bootloader Secure Boot ;
- le bootloader ne se met pas à jour par OTA : le durcissement ne
  concerne que les cartes neuves ou repassées une fois en USB ;
- procédure de récupération testée de bout en bout sur un lot de
  cartes sacrifiables (≥ 10), avant toute carte client.

### D. Réseau et architecture

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-D1 | Connexions sortantes seulement côté device (aucun port entrant) | O | L | — | Fait |
| EX-D2 | Contrôle sans dépendance cloud : tout tourne sur site | I | L | — | Fait (pnex-deploy) |
| EX-D3 | Comportement **fail-safe** défini par sortie en cas de perte de communication | I | L | V2 | Partiel (cartes de régulation D20) |
| EX-D4 | Installation et mises à jour **sans Internet** (bundles signés, catalogue, builder hors ligne, NTP local) | C | L | V3 | Builder hors ligne fait ; le reste absent |
| EX-D5 | Segmentation par zones et conduits (IEC 62443-3-2) documentée, gabarits de flux réseau | C | P | V3 | Absent |
| EX-D6 | Compatibilité diode réseau : remontée de données en sens unique vers l'IT | S | L | V5 | Absent |
| EX-D7 | **Transports filaires** : Ethernet (W5500, LAN8720), RS-485 / Modbus | C opt, S | L+matériel | V5 | WiFi seulement |

### E. Serveur et plateforme

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-E1 | Chiffrement au repos de la base et du stockage objet ; résidence des données | C | L+P | V3 | Coffre de secrets seulement (D110) |
| EX-E2 | Images conteneur durcies (non-root, lecture seule, surface minimale) | I | L | V2 | Partiel |
| EX-E3 | Clés plateforme dans un HSM (PKCS#11) | C | L | V3 | Absent |
| EX-E4 | Sauvegarde **testée** des clés de devices (restauration prouvée) | I | L+P | V3 | Absent — prérequis de toute carte durcie |

### F. Identité et accès

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-F1 | MFA obligatoire, durée de session bornée | I | L | V2 | Possible via Rauthy, non imposé |
| EX-F2 | Fédération AD / LDAP de l'entreprise | I | L | V3 | Absent (Rauthy en amont à évaluer) |
| EX-F3 | Accès « bris de glace » tracé | C | L+P | V3 | Absent |
| EX-F4 | Séparation des rôles : qui écrit un flow ≠ qui le déploie | C | L | V3 | Absent |

### G. Traçabilité

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-G1 | Journal d'audit de toutes les actions (qui, quoi, quand, avant/après) | I | L | V2 | Partiel (journaux par domaine) |
| EX-G2 | Export SIEM (syslog / CEF) | I | L | V2 | Absent |
| EX-G3 | Journal infalsifiable (chaînage de hachés, horodatage) | C | L | V3 | Absent |
| EX-G4 | Piste d'audit au sens FDA 21 CFR Part 11 (signatures électroniques) | C opt | L | V3 | Absent |

### H. Chaîne d'approvisionnement et dossier fournisseur

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-H1 | SBOM publié à chaque release (CycloneDX), serveur et firmware | O | L+P | V2 | `cargo deny` seulement |
| EX-H2 | Releases et images signées | O | L | V2 | Absent |
| EX-H3 | Builds reproductibles | C | L | V3 | Absent |
| EX-H4 | Dossier type pour questionnaire fournisseur (architecture, modèle de menace, mesures) | I | P | V2 | `security.md` interne seulement |
| EX-H5 | Pentest externe périodique | I | P | V3 | Absent |
| EX-H6 | Processus de développement conforme IEC 62443-4-1 | C | P | V3 | Partiel (revue §6, registre §7) |
| EX-H7 | Certification de composant (IEC 62443-4-2, CSPN ANSSI) | C opt, S | P | V5 | Absent |

### I. Vulnérabilités et réglementaire

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-I1 | Politique de divulgation publiée (`security.txt`, contact, délais) | O | P | V2 | Absent |
| EX-I2 | **CRA** : signalement à l'ENISA d'une vulnérabilité activement exploitée (alerte 24 h, notification 72 h, rapport final), en vigueur depuis le 11/09/2026 pour les produits concernés | O | P | V2 | Absent — à qualifier juridiquement (offre commerciale ou non) |
| EX-I3 | Durée de support de sécurité annoncée (CRA : en principe ≥ 5 ans) | O | P | V2 | Absent |
| EX-I4 | Directive radio (EN 18031) si PneX vend des cartes WiFi | — | P | — | Question juridique ouverte (§8) |

### J. Sûreté de fonctionnement et pérennité

| ID | Exigence | Profil | Impact | Vague | État |
|---|---|---|---|---|---|
| EX-J1 | Watchdog et état sûr au redémarrage | I | L | V2 | Partiel |
| EX-J2 | Non-objectif explicite : PneX **n'est pas un système de sécurité** au sens IEC 61508 (pas de fonction SIL) | tous | P | V2 | Non écrit |
| EX-J3 | Plan de continuité si PneX disparaît : code et clés déposés chez un tiers, autonomie totale du self-hosted | C | P | V3 | Self-hosted autonome fait ; dépôt absent |

## 6. Vagues

| Vague | Contenu | Déclencheur | eFuse |
|---|---|---|---|
| **V1** | Protocole industriel pour tous, avant la release (D153–D157) : EX-A1–A6, A10 (couche), B1, B2 | Livrée (2026-10-09, lots L1–L6) | Non |
| **V2** | Profil `industrial` logiciel et dossier : B3, B4, B7, D3, E2, F1, G1, G2, H1, H2, H4, I1–I3, J1, J2 ; mécanisme de flags (§4) | Après V1 | Non |
| **V3** | Profil `critical` logiciel : A7, A8, A10 (transport de la clé), B5, B8, D4, D5, E1, E3, E4, F2–F4, G3, G4, H3, H5, H6, J3 | Premier client industriel réel | Non |
| **V4** | Durcissement matériel C3/S3/C6 : C1–C5, après les prérequis de §5.C | Firmware validé **1 à 2 ans** par la communauté + banc de cartes sacrifiables | **Oui** |
| **V5** | Profil `sovereign` : A9, B6, C6–C8, D6, D7, H7 | Demande d'un client défense/nucléaire, avec budget de certification | Oui |

## 6 bis. Plan de mise en œuvre de V1 (lots, 2026-10-09)

L1 (couche Noise) est livré. Les lots suivants mènent à D153–D158 ; chacun
est livrable seul, testé, et casse volontairement l'existant (pré-release,
D157 : reflash USB de toutes les cartes à la mise à jour).

| Lot | Contenu | Exigences |
|---|---|---|
| L2 ✅ 2026-10-09 | **TLS obligatoire + jeton hors de l'URL.** Le serveur refuse un device qui n'arrive pas par l'edge TLS (`X-Forwarded-Proto: https` posé par nginx, effacé s'il vient du client ; désactivable pour les tests) ; le firmware ne compile plus en `ws://`. Le jeton quitte toutes les URL (`/ws/device`, `/ws/sensor/ingest`, `/ws/camera`, téléchargement OTA) pour l'en-tête `Authorization: Bearer` (client WebSocket maison du firmware, `HTTPClient::addHeader` pour l'OTA, agent edge) : plus de secret dans les journaux d'accès. L'URL ne garde que `device_id` (non secret, unique par org seulement — d'où le jeton tant que L4 n'identifie pas le device par son certificat). | EX-A4, EX-A5, D154 |
| L3 ✅ 2026-10-09 | **PKI par org.** CA ECDSA P-256 par org (créée au premier build, clé dans le coffre, secret d'org) ; au build : clé + certificat du device (CN = `device_id`, SAN = org, validité 10 ans), la clé privée ne vit que dans l'image ; registre `device_certificates` (empreinte SHA-256, série, révocation). | EX-A6, D153 |
| L4 ✅ 2026-10-09 | **Vérification du certificat client.** nginx `ssl_verify_client optional_no_ca` sur `/ws/` du point d'entrée devices (D158) et transmet le certificat (`X-Client-Cert`, effacé côté client) ; le backend vérifie la chaîne contre la CA de l'org du device, l'empreinte contre le registre, la révocation. Sans certificat valide : refus (profil `open` : repli jeton seulement pour une carte marquée « échec HIL », D157). | D153, D158 |
| L5 ✅ 2026-10-09 (NodeMCU OLED + C6-Zero) | **Firmware.** Certificat + clé client compilés (`PNEX_CLIENT_CERT/KEY`) ; ESP32/C3/C6/S3 `WiFiClientSecure::setCertificate/setPrivateKey`, ESP8266 BearSSL `setClientECCert` ; banc HIL : heap 8266 et durée de la poignée de main ECDSA. | EX-A6 |
| L6 | **Agent edge, déploiement, docs.** Agent : certificat client rustls émis à l'enrôlement ; compose/Helm : point d'entrée devices dédié, config nginx ; rotation de CA documentée (rebuild + OTA, EX-A8 en V3) ; notes de release (reflash). | D158, EX-A8 |

## 7. Garde-fous non négociables

1. Aucun eFuse brûlé par défaut, ni par un outil de test, ni « pour
   essayer » (même école que « jamais de flash pour tester »).
2. Jamais sur une carte du banc HIL ni sur une carte de développement.
3. Avant tout eFuse : profil explicite, carte compatible, restauration
   des clés **prouvée** (EX-E4), confirmation qui nomme l'irréversible.
4. Les eFuses brûlés sont des faits enregistrés par device ; aucune
   action ne prétend les annuler.
5. Le download UART n'est jamais désactivé hors `sovereign`.
6. Le profil `open` ne perd jamais une capacité au profit des profils
   supérieurs : flash navigateur, reflash libre, ESP8266.

## 8. Questions ouvertes

1. CRA : PneX est-il un « produit avec éléments numériques » mis sur le
   marché (offre commerciale) ou un logiciel libre exempté ? Qui est
   fabricant quand l'utilisateur assemble sa carte ? Avis juridique
   nécessaire avant V2.
2. Directive radio : obligations si PneX vend un jour des cartes.
3. Firmware : migration ESP-IDF (C++) ou firmware Rust (esp-hal) comme
   socle de V4 ?
4. `sovereign` : quel matériel (élément sécurisé, filaire, automates
   derrière passerelle) ? À trancher avec un client réel.
5. Format d'export et de séquestre des clés (EX-E4, D150).

## 9. Journal

- **2026-10-08** — Création. Revue externe du firmware (lib `pnex`, frame
  D8, OTA) : trous de protocole confirmés dans le code et inscrits au
  registre (SEC-17 à SEC-20). Directive produit : cibler les exigences
  les plus élevées jusqu'au militaire, activables par flags ; rien
  d'irréversible avant 1 à 2 ans de validation communautaire du
  firmware ; implémentation plus tard.
- **2026-10-08 (bis)** — Changement de direction (D153–D157). Question
  de départ : Rauthy pour l'authentification des devices ? Écarté
  (dépendance à chaque reconnexion, limiteur Rauthy face à une flotte
  derrière un NAT, double source de vérité du registre) : le standard
  industriel pour un device est une PKI (X.509 + TLS mutuel), pas un
  IdP. Pré-release, donc bascule directe sans compatibilité : le
  correctif SEC-17 en cours (poignée de main maison « HELLO2 », format
  D8 conservé, migration anti-downgrade `session_frames`) est réécrit
  selon D156–D157. La couche ChaCha est **conservée** pour la
  confidentialité post-quantique (D155), malgré le TLS mutuel.
- **2026-10-08 (ter)** — Couche interne = Noise (D156 mis à jour, choix
  utilisateur : une seule pile crypto pour le lien serveur et le M2M à
  venir). Point d'entrée devices dédié (D158) : `devices.dev.pnex.io`
  en DNS seul, CA de l'instance, port 8883 réservé à MQTT ; proxy
  Cloudflare retiré de la stack (`dev.pnex.io` déjà en direct via le
  wildcard), gardé sur le site vitrine. ESP8266 : TLS mutuel tenté,
  repli D157 seulement si le banc échoue. TLS partout, CA locale en LAN.
  Décisions figées, implémentation L1 lancée.
- **2026-10-08 (quater)** — Lot L1 livré (non commité) : couche interne
  Noise `NNpsk0_25519_ChaChaPoly_SHA256` côté serveur (`/ws/device`,
  `/ws/sensor/ingest`, `/ws/camera`, PING/PONG caméra scellé compris),
  agent edge (`snow`) et firmware (C++ portable : `pnex_noise.cpp` +
  Monocypher 4.0.2 vendu + SHA-256/HMAC maison testés FIPS/RFC 4231).
  Vecteurs dorés générés par Rust, rejoués octet pour octet par le test
  hôte C++. Format D8 supprimé partout ; plus de mode en clair (SEC-19) ;
  le bail anti-clone n'est pris qu'après une poignée de main réussie (un
  jeton volé sans la clé ne bloque plus le vrai device). Close 4011 =
  poignée de main refusée. Les 7 firmwares compilent ; **validation HIL à
  faire par l'utilisateur** (heap 8266, durée du X25519). Reste L2–L6.
- **2026-10-09** — Lots L2 à L5 livrés (non commités) et validés sur le
  banc (NodeMCU V3 OLED, ESP32-C6-Zero) : jeton en en-tête, TLS
  obligatoire, PKI par org, certificat client par build, point d'entrée
  devices dédié (4443 en LAN) vérifié par le backend. Contre-épreuves :
  jeton valide sans certificat → 4014 ; en-têtes de l'edge forgés en
  direct sur le backend → 4013 (SEC-21, trouvé et corrigé en route).
  Reste L6 (agent edge, pnex-deploy compose + Helm).
- **2026-10-09 (soir)** — L6 livré, tout commité (non poussé). Agent edge :
  certificat client émis à l'enrôlement (les anciens révoqués),
  `device_host`/`device_port` rendus à l'agent, client rustls en mTLS ;
  E2E réel OK via 4443. `PNEX_DEVICE_HOST` (nom du point d'entrée devices
  compilé dans les firmwares) : `host[:port]` ou `:port` (LAN), seule
  variable lue par le serveur depuis le contrat strict du soir.
  pnex-deploy : compose + `install.sh` (secret d'edge généré, port 4443),
  chart Helm 0.3.0 (`deviceEdge` : nginx dédié derrière un Service
  LoadBalancer sur `devices.<publicHost>`, CA + certificat générés une fois
  et conservés, épinglés par les firmwares ; certificat public possible
  via `deviceEdge.tls.existingSecret`). Chart rendu et config nginx testée
  en conteneur ; **jamais déployé sur un cluster** — à valider sur
  dev.pnex.io (DNS `devices.*` + IP du LB). Rotation de CA d'org : non
  outillée en V1 (CA 30 ans, certificats devices 10 ans) ; une
  compromission impose aujourd'hui une nouvelle CA + rebuild + flash de
  toute l'org — l'outillage (double CA de transition, OTA) reste EX-A8
  en V3.
- **2026-10-09 (nuit)** — Contrat strict (`2359a27`) : jeton brut en
  en-tête, plus de `device_id` en URL ni de base64 sur le fil, firmware
  wss/https seulement (plus de `WS_SSL` ni de `setInsecure`), signature
  OTA obligatoire. Revalidé sur le banc (8266 + C6, Wi-Fi) : connexion
  mTLS sur 4443 ; sans certificat → 4014 (443 et 4443) ; en-têtes d'edge
  forgés en direct → 4013 ; jeton à l'ancien format base64 → 4001 ; OTA
  signée 1→3 et 2→4 ; rétrogradation forcée refusée par la carte.
