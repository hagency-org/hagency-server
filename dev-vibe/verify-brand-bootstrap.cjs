// Exercise real brand CSS/JS with both existing storage encodings and strict CSP.
// Static bootstrap only; this does not compile or emulate the WASM application.
const fs = require('fs');
const path = require('path');
const { chromium } = require(process.env.HAGENCY_PLAYWRIGHT_MODULE || '../../hagency-cloud/e2e/node_modules/@playwright/test');
const root = path.resolve(__dirname, '..');
const csp = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'";
(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const key of ['theme', 'pasion_theme']) for (const encoding of ['plain', 'json']) for (const theme of ['light', 'dark']) for (const width of [1280, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: theme === 'light' ? 'dark' : 'light' });
      try {
        await context.addInitScript(({ key, encoding, theme }) => localStorage.setItem(key, encoding === 'json' ? JSON.stringify(theme) : theme), { key, encoding, theme });
        const page = await context.newPage();
        await page.route('http://hagency-brand.test/**', route => {
          const name = new URL(route.request().url()).pathname;
          if (name === '/') {
            const head = key === 'pasion_theme' ? fs.readFileSync(path.join(root, 'crates/frontend/branding/pasion-head.html'), 'utf8') : '<link rel="stylesheet" href="/assets/hagency-theme.css"><script src="/assets/hagency-theme.js" data-hagency-storage="theme"></script>';
            return route.fulfill({ status: 200, contentType: 'text/html', headers: { 'Content-Security-Policy': csp }, body: '<html><head>' + head + '</head><body' + (key === 'pasion_theme' ? ' class="hagency-auth"' : '') + '><main>Brand bootstrap regression</main></body></html>' });
          }
          if (['/assets/hagency-theme.css', '/assets/hagency-theme.js', '/assets/hagency-mark.svg'].includes(name)) return route.fulfill({ status: 200, contentType: name.endsWith('.css') ? 'text/css' : name.endsWith('.js') ? 'text/javascript' : 'image/svg+xml', body: fs.readFileSync(path.join(root, 'crates/frontend/public', name.slice(1))) });
          return route.abort();
        });
        await page.goto('http://hagency-brand.test/');
        const result = await page.evaluate(() => ({ theme: document.documentElement.dataset.theme || (document.documentElement.classList.contains('dark') ? 'dark' : document.documentElement.classList.contains('light') ? 'light' : null), background: getComputedStyle(document.documentElement).backgroundColor }));
        if (result.theme !== theme || result.background !== (theme === 'light' ? 'rgb(244, 246, 249)' : 'rgb(15, 23, 42)')) throw Error(JSON.stringify({ key, encoding, theme, width, result }));
        console.log(`PASS ${key} ${encoding} ${theme} ${width}px with strict CSP`);
      } finally { await context.close(); }
    }
  } finally { await browser.close(); }
})().catch(error => { console.error(error.message); process.exitCode = 1; });
