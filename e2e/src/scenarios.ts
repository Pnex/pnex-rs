// Multi-page building blocks shared by specs (and external runners).
import { expect } from '@playwright/test';
import type { Api } from './api.ts';
import { FlowsPage, type FlowEditor } from './pages/flows.ts';
import type { AppShell } from './pages/shell.ts';

/** Memory keys accept letters, digits, "_", "." and "-" (64 max). */
export function memoryKey(prefix: string, name: string): string {
  return `${prefix}.${name}`.replace(/[^\w.-]/g, '_').slice(-64);
}

/**
 * Builds and deploys "Inject (every `everySecs` s) → Json Values
 * {field: value} → Memory write `key`", then waits until the value is in
 * the memory store. Returns the editor, left open on the deployed flow.
 */
export async function deployMemoryFlow(
  app: AppShell,
  api: Api,
  opts: { name: string; key: string; field: string; value: number; everySecs?: number },
): Promise<FlowEditor> {
  const flows = new FlowsPage(app);
  await flows.open();
  const editor = await flows.create(opts.name);
  const inject = app.t('flows-palette-inject');
  const values = app.t('flows-palette-value');
  const memory = app.t('flows-palette-memory-write');

  await editor.select(inject);
  await editor.field(app.t('flows-inject-repeat')).fill(String(opts.everySecs ?? 2));

  await editor.addNode('flows-palette-value');
  await editor.inspector.getByRole('textbox', { name: 'key', exact: true }).fill(opts.field);
  await editor.inspector.getByRole('textbox', { name: 'value', exact: true }).fill(String(opts.value));
  await editor.move(values, 320, 120);

  await editor.addNode('flows-palette-memory-write');
  await editor.inspector.getByRole('textbox', { name: new RegExp(`^${app.t('flows-memory-key')}\\b`) }).fill(opts.key);
  await editor.move(memory, 600, 120);
  await editor.closeInspector();

  await editor.wire(inject, 0, values);
  await editor.wire(values, 0, memory);
  await editor.save();
  await editor.deploy();

  await expect
    .poll(
      async () => {
        const res = await api.post('/memory/values', { refs: [{ key: opts.key, field: opts.field }] });
        return res.results[0]?.value ?? null;
      },
      { timeout: 30_000, message: `memory ${opts.key}#${opts.field}` },
    )
    .toBe(opts.value);
  return editor;
}
