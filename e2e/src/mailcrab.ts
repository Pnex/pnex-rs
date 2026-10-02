// Mailcrab (dev SMTP catcher of compose.yaml): read what PNeX sent.
export const MAILCRAB_URL = (process.env.PNEX_E2E_MAILCRAB_URL ?? 'http://localhost:1080').replace(/\/+$/, '');
/** SMTP endpoint as seen from the pnex-server container. */
export const MAILCRAB_SMTP = { host: process.env.PNEX_E2E_SMTP_HOST ?? 'mailcrab', port: Number(process.env.PNEX_E2E_SMTP_PORT ?? 1025) };

export interface MailSummary {
  id: string;
  subject: string;
  to: { email: string }[];
  from: { email: string };
}

export async function mailsTo(address: string): Promise<MailSummary[]> {
  const res = await fetch(`${MAILCRAB_URL}/api/messages`);
  if (!res.ok) throw new Error(`mailcrab ${res.status}`);
  const all = (await res.json()) as MailSummary[];
  return all.filter((m) => m.to.some((t) => t.email.toLowerCase() === address.toLowerCase()));
}
