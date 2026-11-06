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
