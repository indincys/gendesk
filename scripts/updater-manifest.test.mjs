import assert from "node:assert/strict";
import test from "node:test";
import { REQUIRED, compose, fragment, fragmentName, problems } from "./updater-manifest.mjs";

const url = (name) => `https://github.com/indincys/gendesk/releases/download/v0.39.0/${name}`;

test("each side writes only its own platform keys", () => {
  const mac = fragment({
    platform: "darwin",
    url: url("GenDesk_aarch64.app.tar.gz"),
    signature: "sig-mac\n",
  });
  assert.deepEqual(Object.keys(mac.platforms), ["darwin-aarch64", "darwin-aarch64-app"]);
  assert.equal(mac.platforms["darwin-aarch64"].signature, "sig-mac");
  assert.equal(fragmentName("windows"), "updater-windows.json");
  assert.throws(() => fragment({ platform: "linux", url: "x", signature: "s" }), /未知平台/);
  assert.throws(() => fragment({ platform: "darwin", url: "x", signature: " \n" }), /签名为空/);
});

test("composing both fragments yields the manifest older clients already read", () => {
  const mac = fragment({
    platform: "darwin",
    url: url("GenDesk_aarch64.app.tar.gz"),
    signature: "m",
  });
  const win = fragment({
    platform: "windows",
    url: url("GenDesk_0.39.0_x64-setup.exe"),
    signature: "w",
  });
  const manifest = compose("v0.39.0", [win, mac], new Date("2026-11-09T00:00:00Z"));
  assert.equal(manifest.version, "0.39.0");
  assert.equal(manifest.pub_date, "2026-11-09T00:00:00.000Z");
  assert.deepEqual(Object.keys(manifest.platforms), [
    "darwin-aarch64",
    "darwin-aarch64-app",
    "windows-x86_64",
    "windows-x86_64-nsis",
  ]);
  assert.deepEqual(problems("v0.39.0", manifest), []);
});

test("a manifest with one side missing or the wrong version is reported", () => {
  const mac = fragment({
    platform: "darwin",
    url: url("GenDesk_aarch64.app.tar.gz"),
    signature: "m",
  });
  const half = compose("v0.39.0", [mac]);
  assert.deepEqual(problems("v0.39.0", half), ["缺少平台 windows-x86_64"]);
  assert.ok(problems("v0.40.0", half)[0].includes("不一致"));
  assert.equal(problems("v0.39.0", null).length, 1 + REQUIRED.length);
});
