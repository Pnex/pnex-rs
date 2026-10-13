// Media & Vision Studio nodes in the flow editor (media-vision-studio.md
// D192–D197): zones drawn over the image, tracking ports, caption / fusion
// settings and the decision question editor. The flow is saved and
// validated by the server (no deploy), so no model, camera or LLM provider
// is needed.
import { expect, test } from '../src/fixtures.ts';
import { FlowsPage } from '../src/pages/flows.ts';

test.describe('vision studio', { tag: ['@flows', '@i18n'] }, () => {
  test('zones, tracking, captions, fusion and decisions are configured in the inspector', async ({
    app,
    api,
    page,
    prefix,
    capture,
  }) => {
    const flows = new FlowsPage(app);
    await flows.open();
    const editor = await flows.create(`${prefix} vision studio`);

    // Zones: four clicks on the image, then close and name the zone.
    await editor.addNode('flows-palette-vision-zone');
    await expect(editor.inspector).toContainText(app.t('flows-zone-none'));
    const image = editor.inspector.locator('svg[viewBox="0 0 1 1"]');
    const box = (await image.boundingBox())!;
    for (const [x, y] of [[0.1, 0.5], [0.45, 0.5], [0.45, 0.95], [0.1, 0.95]]) {
      await page.mouse.click(box.x + x * box.width, box.y + y * box.height);
    }
    await editor.inspector.getByRole('textbox', { name: app.t('flows-zone-name') }).fill('dock');
    await editor.inspector.getByRole('button', { name: app.t('flows-zone-close') }).click();
    const row = editor.inspector.locator('li').filter({ hasText: 'dock' });
    await expect(row.getByText(app.tr('flows-zone-points'))).toContainText('4');
    await expect(editor.inspector).not.toContainText(app.t('flows-zone-none'));
    await capture('vision-zone-editor', { caption: 'Image zones: a zone drawn over the camera image' });

    // Tracking: two labelled outputs, defaults tuned for 1 fps.
    await editor.addNode('flows-palette-vision-track');
    await editor.closeInspector();
    await editor.move(app.t('flows-palette-vision-track'), 420, 320);
    const track = await editor.node(app.t('flows-palette-vision-track'));
    expect(track.outs.map((p) => p.label)).toEqual([app.t('flow-port-tracks'), app.t('flow-port-track-events')]);
    await editor.select(app.t('flows-palette-vision-track'));
    await expect(editor.inspector.locator('input[type=number]').nth(1)).toHaveValue('0.2');

    // Caption and fusion: the privacy and trust notes are shown.
    await editor.addNode('flows-palette-vision-caption');
    await expect(editor.inspector).toContainText(app.t('flows-caption-privacy'));
    await editor.addNode('flows-palette-media-fuse');
    await expect(editor.inspector).toContainText(app.t('flows-fuse-trust'));

    // Decision: one choice question, options with descriptions.
    await editor.addNode('flows-palette-text-decide');
    await expect(editor.inspector).toContainText(app.t('flows-decide-none'));
    await editor.inspector.getByRole('button', { name: app.t('flows-decide-add') }).click();
    await expect(editor.inspector.getByRole('textbox', { name: app.t('flows-decide-id') })).toHaveValue('q1');
    const areas = editor.inspector.locator('textarea');
    await areas.nth(0).fill('Does this window describe an incident?');
    await areas.nth(1).fill('incident: someone at risk or damage\nroutine: normal activity');
    await expect(editor.inspector.getByText(app.tr('flows-decide-options-hint'))).toContainText('2');
    await capture('text-decide-questions', { caption: 'Text decision: a typed question with its options' });

    // The server accepts the graph: every new kind validates and projects.
    await editor.save();
    const flows_ = await api.named<{ id: number; name: string }>('/flows', `${prefix} vision studio`);
    const graph = JSON.stringify(await api.get(`/flows/${flows_[0].id}`));
    for (const kind of ['vision_zone', 'vision_track', 'vision_caption', 'media_fuse', 'text_decide']) {
      expect(graph).toContain(kind);
    }
  });
});
