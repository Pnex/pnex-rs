// End-to-end alerting: a deployed flow compares a value to a threshold in a
// Starlark function and, while the alarm is true, sends an email through
// a Notification node (SMTP channel → mailcrab).
import { expect, test } from '../src/fixtures.ts';
import { MAILCRAB_SMTP, mailsTo } from '../src/mailcrab.ts';
import { FlowsPage } from '../src/pages/flows.ts';

test.describe('flow alerting', { tag: ['@flows', '@notifications'] }, () => {
  test('threshold function triggers an email notification', async ({ app, api, page, prefix, capture }) => {
    test.setTimeout(3 * 60_000);
    const rcpt = `${prefix.replace(/[^\w-]/g, '')}-alert@example.com`;
    // Preconditions (their own UIs are covered elsewhere).
    const fn = await api.post<{ id: number; name: string }>('/functions', {
      name: `${prefix} over threshold`,
      language: 'starlark',
      code: [
        '# @input value number "Measured value"',
        '# @input threshold number=20 "Alarm threshold"',
        '# @output alarm bool "True above the threshold"',
        'def handle(inputs, msg):',
        '    return {"alarm": inputs["value"] > inputs["threshold"]}',
      ].join('\n'),
    });
    const channel = await api.post<{ name: string }>('/notify/channels', {
      kind: 'smtp',
      name: `${prefix} on-call mail`,
      enabled: true,
      config: { host: MAILCRAB_SMTP.host, port: MAILCRAB_SMTP.port, tls: 'none', from: 'PNeX <alerts@pnex.local>', to: rcpt },
    });
    const template = await api.post<{ name: string }>('/notify/templates', {
      name: `${prefix} pressure alarm`,
      subject: 'Pressure alarm',
      body: 'The pressure went above its threshold.',
    });

    const flows = new FlowsPage(app);
    await flows.open();
    const editor = await flows.create(`${prefix} pressure alert`);
    const inject = app.t('flows-palette-inject');
    const values = app.t('flows-palette-value');
    const split = app.t('flows-palette-json-split');
    const func = app.t('flows-palette-function');
    const notify = app.t('flows-palette-notify');

    await editor.select(inject);
    await editor.field(app.t('flows-inject-repeat')).fill('5');
    await editor.move(inject, 20, 100);
    await editor.addNode('flows-palette-value');
    await editor.inspector.getByRole('textbox', { name: 'key', exact: true }).fill('value');
    await editor.inspector.getByRole('textbox', { name: 'value', exact: true }).fill('25');
    await editor.move(values, 250, 100);
    await editor.addNode('flows-palette-json-split');
    await editor.move(split, 480, 100);
    await editor.addNode('flows-palette-function');
    await editor.inspector.getByRole('combobox').first().selectOption({ label: `${fn.name} (v1)` });
    await editor.move(func, 710, 100);
    await editor.addNode('flows-palette-notify');
    await editor.inspector.getByRole('button', { name: new RegExp(app.t('flows-notify-add-channel')) }).click();
    // Options read "<name> (<kind>)": pick by the option holding the name.
    const pickBy = async (placeholderKey: string, name: string) => {
      const select = editor.inspector.getByRole('combobox').filter({ has: page.locator('option', { hasText: app.t(placeholderKey) }) });
      const value = await select.locator('option').filter({ hasText: name }).getAttribute('value');
      await select.selectOption(value!);
    };
    await pickBy('flows-notify-pick-channel', channel.name);
    await pickBy('flows-notify-no-template', template.name);
    console.log('NOTIFY-INSPECTOR', await editor.inspector.ariaSnapshot());
    await editor.move(notify, 940, 100);
    await editor.closeInspector();

    await editor.wire(inject, 0, values);
    await editor.wire(values, 0, split);
    await editor.wire(split, 'value', func, 'value');
    await editor.wire(func, 'alarm', notify, 'trigger');
    await capture('flow-alert-graph', { caption: 'Threshold → email alert flow' });
    await editor.save();
    await editor.deploy();

    await expect.poll(async () => (await mailsTo(rcpt)).length, { timeout: 60_000, intervals: [3_000], message: `alert mail to ${rcpt}` }).toBeGreaterThan(0);
    expect((await mailsTo(rcpt))[0].subject).toBe('Pressure alarm');

    await editor.stop();
  });
});
