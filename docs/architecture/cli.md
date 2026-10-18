# CLI d'administration `pnex` (D251–D253)

## 1. Contexte

Le MCP (D245–D250, `mcp.md`) sert un assistant IA : il n'expose ni
suppression, ni action physique, ni deploy/stop. Un administrateur
d'instance a besoin de l'inverse : tout faire depuis un terminal ou un
script, avec ses propres droits. Le serveur reste le seul juge : la CLI
n'ajoute aucun pouvoir, un viewer reçoit toujours son 403.

## 2. Décisions

- **D251 — Un binaire `pnex`, généré du spec embarqué.** Crate
  `crates/pnex-cli`. `docs/contracts/openapi.json` est inclus à la
  compilation (`include_str!`) ; la garde `tests/openapi.rs` le tient égal
  au serveur du même commit. Chaque opération devient
  `pnex <tag> <action>` : le tag du spec, puis l'`operationId` sans le
  préfixe du tag, en kebab-case (`pnex devices delete --id 42`,
  `pnex flows list`, `pnex ontology …`). Chaque paramètre est un flag
  (`--id`, `--limit`…), le corps passe par `--body <json|@fichier|->`.
  `pnex raw <MÉTHODE> <chemin>` reste l'échappatoire. Aucune liste à
  maintenir : une route ajoutée au spec est une commande au build suivant.
  `pnex login` compare `CONTRACT` au serveur et refuse un écart (« installe
  la CLI de la release du serveur »).
- **D252 — Connexion une fois, URL quelconque.** `pnex login <url>` sur
  n'importe quelle instance (open source, auto-hébergée) : code + PKCE S256
  via les proxys de l'instance (`/api/v1/oauth2/sso`, `/token`), retour en
  boucle locale `http://localhost:33418/callback` (le même que les clients
  MCP, déjà déclaré sur le client Rauthy `pnex`, D250). La session
  (URL, org par défaut, jetons) vit dans
  `$XDG_CONFIG_HOME/pnex/session.json` (`%APPDATA%` sous Windows), mode
  `0600`. L'access token est rafraîchi seul dans sa dernière minute ; on ne
  revoit le navigateur qu'après `pnex logout` ou 48 h sans aucun usage
  (durée de vie du refresh token Rauthy), et une session navigateur encore
  ouverte rend ce retour à un clic. `pnex org <id>` fixe l'org par défaut,
  `--org` la surcharge pour un appel.
- **D253 — `--yes` obligatoire pour le dangereux, sans invite.** Toute
  opération `DELETE` et toute opération classée dans `EXCLUDED`
  (`crates/pnex-api-contract/src/exposure.rs`, table partagée avec le MCP)
  sous un motif `destructive`, `physical`, `runtime`, `firmware`,
  `credential`, `access`, `outward`, `sign-off` ou `platform` refuse de
  partir sans `--yes`. `raw` exige `--yes` pour toute méthode autre que
  `GET`. Pas d'invite interactive : le même geste en terminal et en
  script. Un nouveau motif d'`EXCLUDED` se classe dans `DANGEROUS`
  (`crates/pnex-cli/src/main.rs`) ou non, dans le même commit ; le test
  `dangerous_reasons_exist` refuse un motif listé qui n'existe plus.

## 3. Sortie et erreurs

JSON joli sur stdout (à passer dans `jq`), corps binaire tel quel ; une
réponse non 2xx part sur stderr avec code de sortie 1 (`ErrorDetail` du
serveur, non retraduit : outil d'admin, anglais canonique).

## 4. Distribution

`task build:cli [TARGET=<triple>]` → `target/dist/pnex-cli-<triple>.tar.gz`
(`.zip` Windows). Le job `cli` d'`apps.yml` publie Linux x86_64/aarch64
(musl, statique), Windows x86_64 et macOS aarch64 dans la release
`nightly` et chaque tag `v*`. Depuis les sources :
`cargo install --path crates/pnex-cli`.

```sh
pnex login https://pnex.example.org
pnex org 16
pnex devices list --limit 5 | jq '.results[].device_id'
pnex devices delete --id 42 --yes
pnex raw GET '/api/v1/flows?limit=1'
pnex logout
```

## 5. Limites connues

- `pnex logout` oublie la session locale ; le refresh token reste valide
  côté Rauthy jusqu'à son expiration (48 h max), pas de révocation.
- Le port 33418 est partagé avec la connexion des clients MCP : deux
  connexions simultanées se refusent (« port busy »).
- Le binaire macOS n'est pas signé (Gatekeeper : `xattr -d
  com.apple.quarantine pnex`).
