# Mode M2M résilient distribué — PRD

> **Statut : proposition roadmap (2026-09-18) · Portée : nouvelle capacité
> produit, distincte du mode flex existant.** **École volontairement
> distincte de la doctrine serveur-centrique du reste de PNeX** (D44–D48,
> D20, CONTRACT — `edge-model.md`, `control-cards.md`) : autorité locale,
> boucle terrain sans lien serveur, identité/contrat/provisioning propres
> au mesh. Les devices M2M ne sont **pas** des profils D44 (pas de pins,
> pas de build firmware serveur, pas de re-cast) — cf. §2 bis pour le
> contraste doctrine par doctrine.

## 1. Contexte et problème

PNEX dispose déjà d'un **mode flexible** pour l'orchestration inter-devices :

```
iot  ──websocket──▶  EdgeLink (runtime Node-RED/Rust)  ──▶  iot
```

Ce mode est puissant et ouvert, mais il est **centralisé par construction** :
la logique de décision vit dans un moteur central. Conséquence structurelle :
si ce moteur (ou le lien réseau vers lui) tombe, la boucle terrain se fige —
tout bascule en mode sécurité, même quand les devices concernés sont
physiquement côte à côte. C'est un **point de défaillance unique (SPOF)**,
acceptable pour du pilotage souple, **disqualifiant pour du M2M critique**.

Le besoin non couvert : une **boucle sonde → décision → actionneur qui
survit à la perte du serveur**, parce que la décision s'exécute là où sont
les E/S, sans dépendance à un centre.

## 2. Positionnement produit

Deux capacités **complémentaires**, pas concurrentes :

| Axe | Mode flex (existant) | Mode M2M résilient (ce PRD) |
|-----|----------------------|------------------------------|
| Logique de décision | Centralisée (EdgeLink) | **Distribuée, embarquée dans les nœuds** |
| Ouverture | Très ouvert, tout device | **Devices spécialisés, compatibilité stricte** |
| Rôle du hub | Orchestration active | **Config + collecte uniquement** |
| Perte du hub/serveur | Bascule sécurité globale | **Boucle terrain continue (fail-safe local)** |
| Cas d'usage | Prototypage, logique souple, non-critique | M2M réactif résilient, tolérant aux pannes |

Le mode flex reste la réponse par défaut. Le mode M2M résilient est
**volontairement fermé et spécialisé** : peu de types de devices, contrat
d'interopérabilité strict, pour garantir un comportement déterministe et
auditable.

## 2 bis. Une école à part — contraste avec la doctrine serveur-centrique

Le mode flex n'est qu'une instance de la doctrine générale de PNeX :
**serveur-centrique** (lien WS permanent, serveur autoritatif,
observabilité centralisée). Le mode M2M n'est pas une variante de cette
doctrine — c'est une école parallèle. Les mécanismes serveur-centriques ne
sont **pas** portés tels quels :

| Axe | Doctrine serveur-centrique (reste de PNeX) | École M2M (ce PRD) |
|---|---|---|
| Config globale | Le serveur fait autorité sur tout (desired-state, re-cast = conciliation, D45) | Le **hub détient la config globale** (source de vérité) — **cast au changement** + **sync périodique** (fréquence à définir) |
| Logique métier | Exécutée par le serveur / EdgeLink | **Vit dans le nœud** — fonctionne en autonomie complète ; le hub n'exécute rien, sa chute ne fige rien |
| Boucle terrain | Transite par le serveur — même le profil autonome (D45) reste connecté | **Jamais dans la boucle** : P2P direct, hub hors boucle par construction |
| Connectivité | WS permanent + ChaCha20 (D8), backoff, store-and-forward vers le serveur (D46) | Mesh IPv6 (OpenThread) + pub/sub zenoh-pico ; aucun lien serveur requis |
| Identité & provisioning | device_id + token serveur, announce/ack | Identité du mesh (credentials Thread), provisioning zéro-touch propre à cette école (étape 3) |
| Contrat | CONTRACT fil serveur (`proto.rs`), versionnage additif | Contrat de compatibilité M2M **distinct** (clés zenoh, schéma payload) — pas un additif au CONTRACT |
| Observabilité | Séries O2 centralisées, datation serveur (D46) | Best-effort par le hub : le terrain tourne sans télémétrie — perdre le serveur = perdre l'observation, **pas le contrôle** |
| Perte du serveur | Événement de sécurité (safe-states D20) | Non-événement pour la boucle ; fail-safe local **choisi** par règle |

Conséquence pratique : ne pas réutiliser les mécanismes centralisés
(announce/ack, re-cast, ChaCha-over-WS vers le serveur, séries O2 comme
chemin critique) pour les devices M2M. Les points de contact avec le reste
de PNeX restent limités au **rôle hub** (collecte + push config, étape 2)
et à l'**étape 3** (provisioning, OTA, inventaire versions — cf. §9
d'`edge-model.md`).

## 3. Objectifs / non-objectifs

**Objectifs**

- Boucle de contrôle **sans cerveau central** : la décision vit dans le
  nœud qui porte l'action.
- **Résilience** : la perte du hub/serveur ne casse pas la coordination
  terrain.
- **Fail-safe conçu, pas subi** : chaque nœud sait quoi faire s'il perd une
  source ou un pair (timeout, valeur de repli, état de sécurité local).
- Hub = **config + collecte** seulement (le lent, ~1 Hz, y est légitime).
- Devices **spécialisés** avec un contrat de compatibilité strict.

**Non-objectifs**

- Pas de remplacement du bus de contrôle certifié (PROFINET, EtherCAT,
  safety SIL/PL) — hors périmètre, hors responsabilité.
- Pas de temps-réel **dur borné** (le sans-fil ne le garantit pas ;
  positionnement = soft-control / M2M réactif résilient).
- Pas d'ouverture « tout device » : c'est justement le rôle du mode flex.
- Pas d'UI dans le POC.

## 4. Principes d'architecture

1. **La décision s'exécute là où sont les E/S.** Le nœud actionneur (ou
   chaque nœud pour sa part) embarque sa règle et l'applique localement à
   partir des mesures reçues en pair-à-pair.
2. **Deux couches découplées** :
   - **Réseau** — mesh IPv6 auto-cicatrisant (**OpenThread / 802.15.4**) :
     pas de nœud central dont la perte coupe la communication ; re-routage
     automatique.
   - **Applicatif** — pub/sub pair-à-pair (**zenoh-pico**) : les sondes
     publient, l'actionneur souscrit en direct, sans broker au milieu.
3. **Hub non bloquant, détenteur de la config globale** : un Thread
   Border Router détient la **config globale** (source de vérité) —
   **cast au changement** + **sync périodique** (fréquence à définir) —
   et souscrit aux mesures (collecte). La **logique métier vit dans le
   nœud** : entre deux syncs, il applique sa règle en autonomie complète —
   la chute du hub n'interrompt ni les liens P2P ni la régulation.

```
┌─────────────────────────────────────────────┐
│  Règle locale (avg/min/max > seuil → action) │  ← nœud actionneur
├─────────────────────────────────────────────┤
│  zenoh-pico  → pub/sub P2P (qui parle à qui)  │  ← applicatif
├─────────────────────────────────────────────┤
│  OpenThread  → mesh IPv6 (comment ça circule) │  ← réseau
├─────────────────────────────────────────────┤
│  802.15.4    → radio (ESP32-C6 / H2)          │  ← physique
└─────────────────────────────────────────────┘
```

## 5. Devices spécialisés & contrat de compatibilité

Périmètre volontairement restreint. Types de devices de première
génération :

- **Sonde** — publie une mesure typée sur une clé dédiée, à fréquence
  configurable.
- **Actionneur** — souscrit à un ensemble de clés, agrège (avg/min/max),
  applique une règle, pilote une sortie.
- **Passerelle / collecteur** (rôle hub) — Thread Border Router, collecte +
  push de config. Non requis pour que la boucle tourne.

**Compatibilité stricte** : format de clé, schéma de payload et versions de
contrat figés et validés au provisioning. Un device non conforme au contrat
est refusé (pas de best-effort, pas de dégradé silencieux). C'est le prix
du déterminisme et de l'auditabilité.

## 6. Modèle applicatif (clés & messages)

```
pnex/m2m/sensor/<id>/value      → { "v": <float>, "ts": <epoch>, "seq": <n> }
pnex/m2m/actuator/<id>/state    → { "state": "...", "reason": "..." }
pnex/m2m/config/<node>          → règle + params de fail-safe (poussé par le hub)
```

La sync périodique hub ↔ nœud porte la **version de config** (hash + seq)
sur la même clé `config/<node>` : re-cast uniquement en cas de divergence
— sinon le nœud continue sur sa règle embarquée.

Exemple de règle (embarquée dans l'actionneur) :

```json
{
  "metric": "avg",
  "op": ">",
  "threshold": 25.0,
  "action": { "state": "on" },
  "failsafe": {
    "sensor_timeout_ms": 3000,
    "on_missing_source": "hold_last | safe_state",
    "safe_state": "off"
  }
}
```

## 7. Logique de décision & fail-safe

- **Compute local** : moyenne / min / max sur les N sondes souscrites.
- **Règle simple** (seuil/opérateur/action) — pensé « ladder pour codeurs »,
  pas d'IEC 61131-3 dans un premier temps.
- **Fail-safe local, par nœud** :
  - timeout par source → repli défini (dernière valeur / état sûr) ;
  - comportement explicite si un pair se tait ;
  - état de sécurité **choisi**, jamais un blocage global déclenché par la
    chute d'un serveur.

## 8. Phasage

**Étape 1 — POC logique distribuée (sur parc ESP existant, WiFi, zéro achat)**

- zenoh-pico **directement sur WiFi/UDP** (sans OpenThread).
- Valide : pub/sub P2P sans broker, compute local, règle embarquée,
  fail-safe local.
- Limite assumée : pas de mesh, pas de multi-hop, pas d'auto-cicatrisation
  réseau.
- Livrable : sondes + actionneur qui bouclent en direct ; test de coupure
  hub → la boucle continue.

**Étape 2 — Résilience réseau (devices spécialisés ESP32-C6/H2)**

- Ajout d'**OpenThread** sous zenoh-pico → mesh IPv6 auto-cicatrisant +
  multi-hop.
- Thread Border Router comme hub collecte/config.
- Valide : survie à la perte d'un nœud relais, re-routage automatique.

**Étape 3 — Industrialisation**

- Provisioning zéro-touch, gestion/rotation des clés, OTA sécurisé,
  versionnage du contrat de compatibilité.

## 9. Points à valider (incertitudes techniques)

- **zenoh-pico sur 802.15.4/6LoWPAN via OpenThread** : MTU réduit
  (~1280 o, fragmenté), débit faible. OK pour télémétrie + décision (petits
  messages), à **prototyper avant engagement** — principal risque du combo.
- **Découverte P2P zenoh-pico en mode peer pur** sur ESP/WiFi puis sur
  Thread : à mesurer.
- **Latence end-to-end** sonde→décision→actionneur, et sa dispersion
  (jitter) selon fréquence cible.
- **Fréquence cible** de la boucle à arbitrer (conditionne WiFi vs descente
  plus bas dans la stack).
- **Fréquence de sync config** hub ↔ nœud à arbitrer (re-cast uniquement
  sur divergence de version).

## 10. Contraintes matérielles

- Étape 1 : ESP32 / ESP8266 WiFi (parc existant).
- Étape 2+ : **ESP32-C6 ou H2 obligatoires** pour Thread (802.15.4). Coût
  d'entrée matériel assumé pour obtenir le mesh résilient standard et
  ouvert.

## 11. Critères de succès (POC)

- La boucle sonde → décision → actionneur fonctionne **sans passer par un
  centre**.
- **Coupure du hub/serveur** : la boucle terrain **continue** de
  fonctionner.
- **Perte d'une source** : le nœud actionneur applique son fail-safe local
  défini (pas de blocage global).
- Un device non conforme au contrat est **rejeté** au provisioning.
- (Étape 2) Perte d'un nœud relais : le mesh **re-route** et la
  communication survit.
