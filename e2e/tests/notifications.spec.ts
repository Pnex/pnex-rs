// Notification channels: an SMTP channel tested from its form reaches a
// real mailbox (mailcrab), then is created and deleted.
import { expect, test } from '../src/fixtures.ts';
import { MAILCRAB_SMTP, mailsTo } from '../src/mailcrab.ts';
import { NotificationsPage } from '../src/pages/notifications.ts';

test.describe('notifications', { tag: '@notifications' }, () => {
  test('email channel: test mail delivered, create, delete', async ({ app, prefix, capture }) => {
    const name = `${prefix} mail`;
    const rcpt = `${prefix.replace(/[^\w-]/g, '')}@example.com`;
    const notifications = new NotificationsPage(app);
    await notifications.open();

    const form = await notifications.newChannel('smtp');
    await form.name(name);
    await form.field('notify-field-host').fill(MAILCRAB_SMTP.host);
    await form.field('notify-field-port').fill(String(MAILCRAB_SMTP.port));
    await form.field('notify-field-tls').selectOption('none');
    await form.field('notify-field-from').fill('PNeX E2E <e2e@pnex.local>');
    await form.field('notify-field-to').fill(rcpt);
    await capture('email-channel-form', { caption: 'SMTP channel form' });

    await form.test();
    await expect.poll(async () => (await mailsTo(rcpt)).length, { timeout: 30_000, message: `mail to ${rcpt}` }).toBeGreaterThan(0);

    await form.create();
    await expect(notifications.channel(name)).toBeVisible();
    await capture('email-channel-created', { caption: 'Channel list' });
    await notifications.deleteChannel(name);
  });
});
