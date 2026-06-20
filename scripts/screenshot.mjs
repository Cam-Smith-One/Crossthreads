// Capture screenshots of the running Crossthreads web UI.
//
// Prereqs: a running daemon (e.g. `scripts/demo.sh`) and Puppeteer:
//   npm install puppeteer
//   node scripts/screenshot.mjs            # -> ct-search.png, ct-conversation.png
//   CT_HTTP=127.0.0.1:47101 node scripts/screenshot.mjs
//
// Uses Puppeteer's bundled Chromium (independent of any system webview), so it
// works headlessly where a native Tauri window can't.
import puppeteer from "puppeteer";

const base = `http://${process.env.CT_HTTP ?? "127.0.0.1:47101"}/`;
const query = process.env.CT_QUERY ?? "my login keeps failing repeatedly";

const browser = await puppeteer.launch({
  headless: "new",
  args: ["--no-sandbox", "--disable-setuid-sandbox", "--disable-gpu"],
});
try {
  const page = await browser.newPage();
  await page.setViewport({ width: 1000, height: 820, deviceScaleFactor: 2 });
  await page.goto(base, { waitUntil: "networkidle0", timeout: 30000 });

  await page.waitForSelector(".searchbar input");
  await page.type(".searchbar input", query);
  await page.select(".searchbar select", "semantic");
  await page.click(".searchbar button[type=submit]");
  await page.waitForSelector(".hit", { timeout: 15000 });
  await new Promise((r) => setTimeout(r, 600));
  await page.screenshot({ path: "ct-search.png" });
  console.log("wrote ct-search.png");

  await page.click(".hit");
  await page.waitForSelector(".modal", { timeout: 10000 });
  await new Promise((r) => setTimeout(r, 500));
  await page.screenshot({ path: "ct-conversation.png" });
  console.log("wrote ct-conversation.png");
} finally {
  await browser.close();
}
