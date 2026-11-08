# Recette locale des runs d'agents (D760)

Chaîne complète sur le poste, avant le cluster de recette (D753), sans faux runner : le
serveur de dev, un vrai runner dans un vrai Kubernetes (`kind`), un vrai build BuildKit, un
vrai bac. Tout passe par la CLI `pnex`.

## 1. Pièces

| Pièce | Où | Note |
|---|---|---|
| Cluster | `kind` v0.29, nommé `pnex-agents`, kubeconfig `~/.kube/config-kind-pnex-agents` | kindnet applique les `NetworkPolicy`, avec quelques secondes de délai (SEC-E4) |
| Registre | conteneur `registry:3` nommé `kind-registry` sur le réseau `kind`, exposé en `127.0.0.1:5001` | containerd du nœud : `config_path` ajouté, `hosts.toml` pour `localhost:5001` et `kind-registry:5000` en HTTP |
| Runner | namespace `pnex-agents-test`, `deploy/runner/runner.yaml` | `PNEX_RUNNER_SERVER_URL=http://<passerelle du réseau kind>:5160`, `PNEX_RUNNER_ALLOW_HTTP=1`, `PNEX_RUNNER_BUILDS=true`, `PNEX_RUNNER_JOB_EGRESS=<sous-réseau kind>` |
| Serveur | `target/debug/pnex-server start --server-and-worker -b 0.0.0.0 -p 5160`, lancé depuis `crates/pnex-backend` | base `pnex_agents_dev` ; la stack en place (`:5150`) n'est pas touchée |
| Images de test | `pnex/sandbox-init` (Wolfi + git + curl), `pnex/image-builder` (BuildKit rootless) | jamais livrées : seulement pour la recette |
| Fournisseur LLM | faux fournisseur Anthropic local, ou les vraies clés de D751 | déclaré par `pnex ai providers-create`, usage `agent` |

Variables du serveur : `PNEX_AGENT_GATEWAY_URL` (adresse joignable depuis les pods),
`PNEX_SANDBOX_INIT_IMAGE` et `PNEX_IMAGE_BUILDER` épinglées par digest,
`OPENOBSERVE_ROOT_EMAIL` et `OPENOBSERVE_ROOT_PASSWORD` de la stack pour les journaux.

## 2. Session CLI sans navigateur

Jeton obtenu par mot de passe auprès du Rauthy de la stack (compte de test), puis rangé
dans un dossier de session à part (`XDG_CONFIG_HOME`). La session principale de `pnex`
(une autre instance) n'est jamais écrasée.

## 3. Déroulé

1. `pnex runners join-tokens-create`, puis le Secret `pnex-runner-join` et le déploiement
   du runner.
2. `pnex image-registries create` : registre `kind-registry:5000`, sans TLS, avec un secret
   d'écriture factice, parce que le registre local n'authentifie pas.
3. `pnex environments create`, puis `versions-create` (Dockerfile et `agent.sh` en
   base), `builds-create`, et enfin `builds-activate`.
4. `pnex ai providers-create`, usage `agent`.
5. `pnex agent-runs agent-artifacts-upload --name ticket`.
6. `pnex agent-runs create`, avec `inputs` et `outputs`.
7. `pnex agent-runs get`, `logs`, `artifacts`, puis `agent-artifacts-content`.

## 4. Ce que l'agent de test prouve

Il n'embarque aucun moteur, puisque le moteur est le choix de l'org (D750). Il :
- lit son entrée ;
- imprime exprès son jeton, qui doit apparaître masqué ;
- appelle le modèle par la passerelle ;
- tente une sortie interdite, qui doit être refusée ;
- écrit un fichier et un dossier ;
- pose un lien symbolique vers `/etc/passwd`, qui doit être ignoré ;
- laisse une sortie déclarée absente, qui doit être marquée `missing`.

## 5. Recette sur le cluster (D753, 2026-11-07)

Même déroulé, sur le cluster partagé, namespace `pnex-agents-test` seul.

| Pièce | Où |
|---|---|
| Serveur | release `pnex-acc` du chart `pnex` (`pnex-deploy`, copie propre de son `HEAD`), image `main-9d86cbc`, `https://agents-test.pnex.io` (TLS chez Cloudflare), sa propre base, son propre RustFS ; `api.extraEnv` porte `PNEX_AGENT_GATEWAY_URL` et les images de test épinglées |
| Runner | release `pnex-runner` du chart `pnex-runner` (D764), jeton d'inscription dans le Secret `pnex-runner-join` |
| Registre | Scaleway, dépôt `pnex/agents-recette` |
| Session CLI | connexion du navigateur scriptée sans écran, dossier de session à part |

Résultats du premier run (agent de test sans moteur, qui échoue exprès en code 7) :
- build BuildKit poussé sur Scaleway en 80 s, puis activé ;
- entrée `ticket` vérifiée : même SHA-256 vu par l'agent ;
- jeton et clé imprimés exprès : `[masked]` dans le journal ;
- sorties `patch` (fichier) et `report` (tar) remontées après l'échec, lien vers `/etc/passwd` absent du tar, `notes` en `missing` ;
- egress refusé vers l'API Kubernetes (SEC-E5 fermé sur ce cluster), vers la base du serveur et vers l'internet public hors claim ;
- appel du modèle : 503, aucun fournisseur saisi à ce stade.

Trouvé en route : l'image d'init de test sans `curl` ouvrait la porte d'egress sans sonder (SEC-E7, corrigé).

La démonstration de D752 a suivi le 2026-11-08 (§6).

## 6. Démonstration de D752 (dev.pnex.io, 2026-11-08)

Un agent hébergé corrige le test rouge du dépôt privé `agent-bugfix-demo` (GitHub) et ouvre
la demande de fusion n° 1.

| Pièce | Où |
|---|---|
| Serveur | `dev.pnex.io`, org 3 ; `api.extraEnv` porte `PNEX_SANDBOX_INIT_IMAGE` et `PNEX_IMAGE_BUILDER` épinglées par digest (`pnex-deploy/helm/values-dev.yaml`, hors git) |
| Runner | chart `pnex-runner` dans un cluster kind local, inscrit sur l'org 3 ; `jobEgress` vers le registre kind |
| Registre | `kind-registry:5000`, dépôt `agents-demo` |
| Environnement | `bugfix-demo` : `wolfi-base` épinglé, `nodejs`, `curl`, `jq`, et `agent.sh` |
| Agent | `bugfix-demo`, niveau hébergé, rôle `viewer` ; dépôt en `/workspace/app` avec `secret_id` et `push_secret_id` sur le même jeton fin (Contents et Pull requests en écriture, ce seul dépôt) ; sortie `report` requise |
| Modèle | fournisseur `together` de l'org, usage `agent` seul, `openai/gpt-oss-120b` |

`agent.sh` joue `node --test`, envoie `csv.js`, le test et la sortie au modèle par la passerelle
(`$OPENAI_BASE_URL`, modèle imposé), remplace `csv.js`, rejoue les tests et écrit le rapport
(avant, après, diff). Le run réussi : 1 appel, 695 jetons en entrée et 368 en sortie, tests verts,
branche `pnex/run-<id>` poussée par le sidecar, demande de fusion ouverte par le serveur et
listée dans `merge_requests`.

Trouvé en route :

- **Image non épinglée refusée** : avec un tag, `builds-create` répond 409
  `environment-build-unavailable` (« no builder image »). Le serveur exige `@sha256:` ;
  le message ne le dit pas.
- **Egress résolu du mauvais côté** : le serveur résolvait l'hôte du dépôt depuis son réseau ;
  GitHub répond par région, et le clone visait depuis kind une adresse absente de la règle
  (clone en délai dépassé). Corrigé : le runner ajoute ses propres réponses DNS pour les dépôts
  et la passerelle (`resolve_here`, `crates/pnex-runner/src/lib.rs`).
- **Modèles** : `lfm3` sous Ollama en CPU (19 jetons/s, raisonnement long) dépasse les 90 s du
  fournisseur ; chez Together, `Llama-3.3-70B-Instruct-Turbo` n'est pas serverless et les
  Qwen 3.7/3.8 exigent le streaming.
- **Jeton GitHub** : un jeton qui ne couvre pas le dépôt fait échouer le clone en 403 (« Write
  access to repository not granted », même en lecture) ; en lecture seule, c'est le push qui
  échoue.

Reste ouvert :

- clone en délai dépassé sur 2 runs sur 8, alors que l'adresse de GitHub figure dans la règle ;
  piste non vérifiée : règle d'autorisation posée par kindnet après la porte d'egress ;
- un push refusé laisse le run « succeeded », l'échec n'est que dans le journal ;
- titre de la demande de fusion générique (« Agent run <id> »).
