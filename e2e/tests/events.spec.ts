// Event log node: a deployed flow writes JSON events that show up on the
// Events page (OpenObserve storage).
import { expect, test } from '../src/fixtures.ts';
import { FlowsPage } from '../src/pages/flows.ts';

test.describe('events', { tag: '@events' }, () => {
  test('flow event log → Events page', async ({ app, page, prefix, capture }) => {
    test.setTimeout(3 * 60_000);
    const flowName = `${prefix} alarms`;
    const message = `${prefix} pressure high`;
    const flows = new FlowsPage(app);
    await flows.open();
    const editor = await flows.create(flowName);
    const inject = app.t('flows-palette-inject');
    const values = app.t('flows-palette-value');
    const eventLog = app.t('flows-palette-event-log');

    await editor.select(inject);
    await editor.field(app.t('flows-inject-repeat')).fill('3');
    await editor.addNode('flows-palette-value');
    await editor.inspector.getByRole('textbox', { name: 'key', exact: true }).fill('pressure');
    await editor.inspector.getByRole('textbox', { name: 'value', exact: true }).fill('7.8');
    await editor.move(values, 320, 120);
    await editor.addNode('flows-palette-event-log');
    await editor.inspector.getByRole('combobox', { name: app.t('flows-event-log-level') }).selectOption({ label: app.t('events-level-warn') });
    await editor.inspector.getByRole('textbox', { name: new RegExp(`^${app.t('flows-event-log-message')}\\b`) }).fill(message);
    await editor.move(eventLog, 600, 120);
    await editor.closeInspector();
    await editor.wire(inject, 0, values);
    await editor.wire(values, 0, eventLog);
    await editor.save();
    await editor.deploy();

    // Ingestion is asynchronous: refresh until the event is listed.
    await expect
      .poll(
        async () => {
          await app.goto('/events');
          return page.getByRole('main').getByText(message).count();
        },
        { timeout: 120_000, intervals: [5_000] },
      )
      .toBeGreaterThan(0);
    await capture('events-page', { caption: 'Events written by the flow' });

    await flows.open();
    const ed = await flows.openFlow(flowName);
    await ed.stop();
    await ed.back();
    await flows.delete(flowName);
  });
});
