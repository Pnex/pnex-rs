// Vision Lab improvement loop (media-vision-studio.md D203–D206): frames of
// an uploaded video, box interpolation between annotated frames, a
// Python-free training started from the panel, and a library file used as
// a stream source. Needs `ffmpeg` on the test machine (synthetic clip) and
// PNEX_VISION_TEST_DIR with yolox_nano.onnx for the training scenario.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from '../src/fixtures.ts';
import { makePng } from '../src/fixtures-files.ts';

/** 10 s H.264 MP4 with a moving box and a tone, `moov` at the end. */
function makeClip(): Buffer {
  const dir = mkdtempSync(join(tmpdir(), 'pnex-e2e-clip-'));
  const out = join(dir, 'clip.mp4');
  execFileSync('ffmpeg', [
    '-y', '-loglevel', 'error',
    '-f', 'lavfi', '-i', 'testsrc=duration=10:size=320x240:rate=10',
    '-f', 'lavfi', '-i', 'sine=frequency=440:duration=10',
    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-f', 'mp4', out,
  ]);
  return readFileSync(out);
}

test.describe('vision lab loop', { tag: ['@lab'] }, () => {
  test('frames of a video and interpolated boxes', async ({ app, api, page, prefix, capture }) => {
    const clip = `${prefix} clip`;
    await api.upload(`/media?filename=clip.mp4&name=${encodeURIComponent(clip)}&content_type=video%2Fmp4`, makeClip());
    const project = await api.post('/lab/projects', { name: `${prefix} carts`, classes: ['cart'] });

    await app.goto(`/lab/${project.id}`);
    await page.getByRole('button', { name: app.t('lab-add-images') }).click();
    await page.getByRole('dialog').getByRole('button', { name: new RegExp(app.t('lab-add-videos')) }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByLabel(clip, { exact: true }).check();
    await dialog.getByRole('spinbutton').nth(1).fill('10');
    await dialog.getByRole('spinbutton').nth(2).fill('2');
    await capture('lab-video-import', { caption: 'Vision Lab: pictures taken from an uploaded video' });
    await dialog.getByRole('button', { name: app.t('lab-rec-import') }).click();

    await expect.poll(async () => (await api.list(`/lab/projects/${project.id}/images`)).length).toBe(5);
    const images: { id: string; frame_ms: number; version: number }[] = await api.list(
      `/lab/projects/${project.id}/images`,
    );
    images.sort((a, b) => a.frame_ms - b.frame_ms);
    for (const [i, x] of [[0, 0.1], [4, 0.5]] as const) {
      await api.put(`/lab/images/${images[i].id}/annotations`, {
        boxes: [{ class: 0, bbox: [x, 0.2, 0.2, 0.2] }],
        expected_version: images[i].version,
      });
    }

    await app.goto(`/lab/${project.id}`);
    await page.getByRole('button', { name: new RegExp(`${clip} 00:04`) }).click();
    // Automatic labeling sits in a folded panel under the timeline.
    await page.getByText(app.t('lab-auto-title'), { exact: true }).click();
    await page.getByRole('button', { name: app.t('lab-interpolate') }).click();
    await expect(page.getByText(`cart · ${app.t('lab-interpolated')}`)).toBeVisible();
    await capture('lab-interpolate', { caption: 'Vision Lab: a box interpolated between two annotated frames' });
  });

  test('training from the panel registers a model', async ({ app, api, page, browser, prefix, capture }) => {
    const dir = process.env.PNEX_VISION_TEST_DIR ?? '';
    const onnxPath = join(dir, 'yolox_nano.onnx');
    test.skip(!dir || !existsSync(onnxPath), 'PNEX_VISION_TEST_DIR with yolox_nano.onnx needed');
    const asset = await api.upload(
      `/media?filename=yolox_nano.onnx&name=${encodeURIComponent(`${prefix} nano`)}`,
      readFileSync(onnxPath),
    );
    const base = await api.post('/ml/models', { name: `${prefix} COCO nano`, asset_id: asset.id });
    const project = await api.post('/lab/projects', { name: `${prefix} signs`, classes: ['sign'] });
    const ids: string[] = [];
    for (const n of [1, 2, 3, 4, 5, 6]) {
      const up = await api.upload(
        `/media?filename=s${n}.png&name=${encodeURIComponent(`${prefix} s${n}`)}&kind=photo`,
        await makePng(browser, `SIGN ${n}`),
      );
      ids.push(up.id);
    }
    const added: { images: { id: string; version: number }[] } = await api.post(`/lab/projects/${project.id}/images`, {
      asset_ids: ids,
    });
    for (const img of added.images) {
      await api.put(`/lab/images/${img.id}/annotations`, {
        boxes: [{ class: 0, bbox: [0.3, 0.4, 0.4, 0.2] }],
        split: 'train',
        expected_version: img.version,
      });
    }
    await api.post(`/lab/projects/${project.id}/versions`, {});

    await app.goto(`/lab/${project.id}`);
    const panel = page.locator('section').filter({ hasText: app.t('lab-train-title') });
    await panel.getByRole('combobox').nth(1).selectOption({ label: base.name });
    await panel.getByRole('spinbutton').fill('3');
    await panel.getByRole('button', { name: app.t('lab-train-start') }).click();
    await expect(panel.getByText(app.t('lab-run-succeeded'))).toBeVisible({ timeout: 180_000 });
    await expect(panel.getByRole('link', { name: app.t('lab-run-open-models') })).toBeVisible();
    await capture('lab-training', { caption: 'Vision Lab: a training finished, the new model is in Models' });
  });

  test('a library file as a stream source', async ({ app, api, page, prefix, capture }) => {
    const clip = `${prefix} replay`;
    await api.upload(`/media?filename=replay.mp4&name=${encodeURIComponent(clip)}&content_type=video%2Fmp4`, makeClip());
    await app.goto('/streams');
    await page.getByRole('button', { name: app.t('streams-add') }).first().click();
    await page.locator('#stream-kind').selectOption('media_asset');
    await expect(page.locator('#stream-file')).toBeVisible();
    await page.locator('#stream-file').selectOption({ label: clip });
    await capture('streams-file-source', { caption: 'Streams: a video of the media library replayed as a source' });
  });
});
