// A small WebDriver client for geckodriver + headless Firefox. No packages:
// just fetch against geckodriver's HTTP API.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import { createServer } from "node:net";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function freePort() {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

export async function startBrowser({ width = 1100, height = 1000 } = {}) {
  const port = await freePort();
  const driver = spawn("geckodriver", ["--port", String(port)], { stdio: "ignore" });
  const root = `http://127.0.0.1:${port}`;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await fetch(`${root}/status`);
      break;
    } catch {
      await sleep(100);
    }
  }
  const call = async (method, path, body) => {
    const response = await fetch(`${root}${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const parsed = await response.json();
    if (!response.ok) throw new Error(`${method} ${path}: ${parsed.value?.message ?? response.status}`);
    return parsed.value;
  };
  const created = await call("POST", "/session", {
    capabilities: {
      alwaysMatch: { browserName: "firefox", "moz:firefoxOptions": { args: ["-headless"] } },
    },
  });
  const base = `/session/${created.sessionId}`;
  await call("POST", `${base}/window/rect`, { width, height });

  const run = (script, ...args) => call("POST", `${base}/execute/sync`, { script, args });
  const browser = {
    run,
    async goto(url) {
      await call("POST", `${base}/url`, { url });
    },
    async path() {
      return run("return location.pathname + location.search;");
    },
    async text() {
      return run("return document.body.innerText;");
    },
    /** Stores a localStorage key on the page's origin (the page must be loaded). */
    async setStorage(key, value) {
      await run("localStorage.setItem(arguments[0], arguments[1]);", key, value);
    },
    async clearStorage() {
      await run("localStorage.clear();");
    },
    async clickButton(label) {
      const clicked = await run(
        `const wanted = arguments[0];
         const button = [...document.querySelectorAll("button, a")]
           .find((el) => el.textContent.trim() === wanted || el.textContent.trim().startsWith(wanted));
         if (!button) return false;
         button.click();
         return true;`,
        label
      );
      if (!clicked) throw new Error(`no button or link labelled "${label}"; page says:\n${await browser.text()}`);
    },
    /** Types into the input matched by `selector`, firing the events Leptos listens to. */
    async fill(selector, value) {
      const filled = await run(
        `const input = document.querySelector(arguments[0]);
         if (!input) return false;
         input.focus();
         const proto = input.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
         Object.getOwnPropertyDescriptor(proto, "value").set.call(input, arguments[1]);
         input.dispatchEvent(new Event("input", { bubbles: true }));
         return true;`,
        selector,
        value
      );
      if (!filled) throw new Error(`no input matches ${selector}; page says:\n${await browser.text()}`);
    },
    /** Polls until the page text contains `needle` (a string or RegExp). */
    async waitForText(needle, { timeout = 15000 } = {}) {
      const deadline = Date.now() + timeout;
      let last = "";
      while (Date.now() < deadline) {
        last = await browser.text();
        if (typeof needle === "string" ? last.includes(needle) : needle.test(last)) return last;
        await sleep(150);
      }
      throw new Error(`timed out waiting for ${needle}; page says:\n${last}`);
    },
    async screenshot(file) {
      const encoded = await call("GET", `${base}/screenshot`);
      writeFileSync(file, Buffer.from(encoded, "base64"));
    },
    async close() {
      try {
        await call("DELETE", base);
      } finally {
        driver.kill();
      }
    },
  };
  return browser;
}

export { sleep };
