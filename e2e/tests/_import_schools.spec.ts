// Dev helper (PNEX_E2E_EXPLORE=1): loads the open data of French schools
// (data.education.gouv.fr annuaire + main higher education institutions)
// as sites of a dedicated org, through the API, to load-test the map.
// SCHOOLS_DIR = folder of the two GeoJSON exports; SCHOOLS_LIMIT = trial size.
import { test } from '@playwright/test';
import { Api, ApiError } from '../src/api.ts';
import { passwordGrant } from '../src/auth.ts';
import { ensureOrg } from '../src/org.ts';

const DIR = process.env.SCHOOLS_DIR ?? '';
const LIMIT = Number(process.env.SCHOOLS_LIMIT ?? Infinity);
const ORG = 'Établissements scolaires';
const PARALLEL = 12;
const EMOJI: Record<string, string> = { Ecole: '🏫', Collège: '📘', Lycée: '🎓', EREA: '🏫', Supérieur: '🏛️' };

interface Site {
  label: string;
  emoji: string;
  latitude: number;
  longitude: number;
  location_detail: string;
  metadata: Record<string, unknown>;
}

async function load(): Promise<Site[]> {
  const fs = await import('node:fs/promises');
  const school = JSON.parse(await fs.readFile(`${DIR}/annuaire-education.geojson`, 'utf8'));
  const higher = JSON.parse(await fs.readFile(`${DIR}/enseignement-superieur.geojson`, 'utf8'));
  const sites: Site[] = [];
  for (const f of school.features) {
    const p = f.properties;
    if (!f.geometry || !(p.type_etablissement in EMOJI) || p.etat !== 'OUVERT') continue;
    const [longitude, latitude] = f.geometry.coordinates;
    sites.push({
      label: p.nom_etablissement,
      emoji: EMOJI[p.type_etablissement],
      latitude,
      longitude,
      location_detail: [p.adresse_1, `${p.code_postal ?? ''} ${p.nom_commune ?? ''}`.trim()].filter(Boolean).join(', '),
      metadata: { uai: p.identifiant_de_l_etablissement, type: p.type_etablissement, statut: p.statut_public_prive },
    });
  }
  for (const f of higher.features) {
    const p = f.properties;
    if (!f.geometry) continue;
    const [longitude, latitude] = f.geometry.coordinates;
    sites.push({
      label: p.uo_lib,
      emoji: EMOJI.Supérieur,
      latitude,
      longitude,
      location_detail: [p.adresse_uai, `${p.code_postal_uai ?? ''} ${p.com_nom ?? ''}`.trim()].filter(Boolean).join(', '),
      metadata: { uai: p.uai, type: 'Supérieur', statut: p.secteur_d_etablissement },
    });
  }
  return sites.slice(0, LIMIT);
}

test('import French schools as sites', async ({ request }) => {
  test.setTimeout(4 * 60 * 60_000);
  const sites = await load();
  const root = new Api(request, (await passwordGrant(request)).access);
  const orgId = await ensureOrg(root, ORG, process.env.SCHOOLS_FRESH === '1');
  let api = root.forOrg(orgId);
  // A long run outlives the access token: grant a new one on the first 401.
  let regrant: Promise<void> | null = null;
  const renew = () =>
    (regrant ??= passwordGrant(request).then((t) => {
      api = new Api(request, t.access, orgId);
      regrant = null;
    }));
  let done = 0;
  const failed: string[] = [];
  const started = Date.now();
  let next = 0;
  await Promise.all(
    Array.from({ length: PARALLEL }, async () => {
      while (next < sites.length) {
        const site = sites[next++];
        try {
          try {
            await api.post('/pois', site);
          } catch (e) {
            if (!(e instanceof ApiError) || e.status !== 401) throw e;
            await renew();
            await api.post('/pois', site);
          }
        } catch (e) {
          if (failed.length < 20) failed.push(`${site.label}: ${(e as Error).message.slice(0, 200)}`);
          else failed.push('');
        }
        done++;
        if (done % 2000 === 0) {
          const rate = done / ((Date.now() - started) / 1000);
          console.log(`${done}/${sites.length} (${rate.toFixed(0)}/s)`);
        }
      }
    }),
  );
  console.log(`imported ${done - failed.length}/${sites.length} in ${((Date.now() - started) / 1000).toFixed(0)} s`);
  console.log(`failed: ${failed.length}\n${failed.filter(Boolean).join('\n')}`);
});
