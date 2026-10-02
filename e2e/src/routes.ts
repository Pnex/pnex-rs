// Authenticated app routes and the ftl key of the h1 each one renders.
// Keep in sync with `Route` in crates/pnex-frontend/src/app.rs.
export interface RouteCase {
  path: string;
  /** ftl key of the page h1; `null` when the page has none. */
  title: string | null;
}

export const ROUTES: RouteCase[] = [
  { path: '/', title: 'nav-dashboard' },
  { path: '/visualisation', title: 'nav-quick-charts' },
  { path: '/map', title: 'viz-map-title' },
  { path: '/dashboards', title: 'nav-dashboards' },
  { path: '/media', title: 'media-title' },
  { path: '/cameras', title: 'cameras-title' },
  { path: '/models', title: 'models-title' },
  { path: '/events', title: 'events-title' },
  { path: '/studio', title: 'nav-studio' },
  { path: '/annotations', title: 'annot-page-title' },
  { path: '/devices', title: 'nav-devices' },
  { path: '/mixtures', title: 'mixtures-title' },
  { path: '/flows', title: 'nav-flows' },
  { path: '/functions', title: 'nav-functions' },
  { path: '/firmware', title: 'nav-firmware' },
  { path: '/notifications', title: 'nav-notifications' },
  { path: '/catalog', title: 'nav-catalog' },
  { path: '/edges/refs', title: 'edgerefs-title' },
  { path: '/orgs', title: 'orgs-title' },
  { path: '/orgs/current', title: null },
  { path: '/secrets', title: 'secrets-title' },
  { path: '/profile', title: 'nav-profile' },
  { path: '/system', title: 'system-title' },
  { path: '/admin/status', title: 'admin-status-title' },
];
