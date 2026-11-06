import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher } from '../src/document-channel.mjs';

const urls = process.argv.slice(2);
if (!urls.length) throw Error('Supply public site URLs to probe');
const browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || '/usr/bin/chromium', headless: true, chromiumSandbox: true });
try {
  for (const url of urls) {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const source = new BrowserDocument(page);
    try {
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 20000 });
      await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});
      const start = performance.now();
      const snapshot = await source.capture();
      const publisher = new DocumentPublisher();
      const packet = publisher.publish(snapshot);
      const followup = publisher.publish(await source.capture());
      console.log(JSON.stringify({ url, title: await page.title(), profile: packet.profile, nodes: packet.put.length, snapshotBytes: Buffer.byteLength(JSON.stringify(packet)), updateBytes: Buffer.byteLength(JSON.stringify(followup)), captureMs: Math.round(performance.now() - start), unsupported: snapshot.unsupported, video: await page.locator('video').count(), physicalDevice: false }));
    } catch (error) {
      console.log(JSON.stringify({ url, error: String(error), sizes: source.metrics }));
      process.exitCode = 1;
    } finally { source.dispose(); await page.close(); }
  }
} finally { await browser.close(); }
