import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { test } from "node:test";

const workflow = readFileSync(new URL("../workflows/release.yml", import.meta.url), "utf8");
const targets = [
  "universal",
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64",
  "linux-x64",
  "win32-arm64",
  "win32-x64",
];
const providers = [
  { name: "VS Code Marketplace", secret: "VSCE_PAT", mode: "VSCE_AUTH_MODE", flag: "--oidc" },
  { name: "Open VSX", secret: "OVSX_PAT", mode: "OVSX_AUTH_MODE", flag: "--trusted-publishing" },
];

const publishStep = (name) => {
  const start = workflow.indexOf(`      - name: Publish every target to ${name}\n`);
  assert.notEqual(start, -1);
  const next = workflow.indexOf("\n  vscode-", start);
  return workflow.slice(start, next === -1 ? undefined : next);
};

const execute = (t, provider, mode, identity = true, pat = true, extraEnv = {}) => {
  const cwd = mkdtempSync(join(tmpdir(), "fallow-publisher-auth-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, "bin"));
  mkdirSync(join(cwd, "vscode-targets"));
  const files = targets.map((target) => `fallow-vscode-1.2.3-${target}.vsix`);
  writeFileSync(
    join(cwd, "vscode-targets/inventory.json"),
    JSON.stringify({ entries: files.map((file) => ({ file })) }),
  );
  for (const file of files)
    writeFileSync(join(cwd, "vscode-targets", file), "prepared fixture bytes");
  writeFileSync(
    join(cwd, "bin/npx"),
    `#!${process.execPath}\nimport { appendFileSync } from "node:fs";\nappendFileSync("calls.jsonl", JSON.stringify({ args: process.argv.slice(2), patPresent: Boolean(process.env[${JSON.stringify(provider.secret)}]), azureSecretPresent: ['AZURE_CLIENT_SECRET', 'AZURE_CLIENT_CERTIFICATE_PATH', 'AZURE_CLIENT_CERTIFICATE_PASSWORD'].some((name) => Boolean(process.env[name])) }) + "\\n");\n`,
    { mode: 0o755 },
  );
  const step = publishStep(provider.name);
  const block = step.match(/        run: \|\n([\s\S]+)/u);
  assert.ok(block);
  const script = block[1]
    .split("\n")
    .map((line) => line.slice(10))
    .join("\n");
  const result = spawnSync("bash", ["-euo", "pipefail", "-c", script], {
    cwd,
    encoding: "utf8",
    env: {
      HOME: cwd,
      PATH: `${cwd}/bin:${dirname(process.execPath)}:/opt/homebrew/bin:/usr/bin:/bin`,
      RELEASE_TAG: "v1.2.3",
      GITHUB_ACTIONS: "true",
      [provider.mode]: mode,
      [provider.secret]: pat ? "synthetic-legacy-pat" : "",
      ACTIONS_ID_TOKEN_REQUEST_URL: identity
        ? "https://token.actions.githubusercontent.com/synthetic"
        : "",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: identity ? "synthetic-oidc-request" : "",
      ...extraEnv,
    },
  });
  let calls = [];
  try {
    calls = readFileSync(join(cwd, "calls.jsonl"), "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  return { result, calls, files };
};

for (const provider of providers) {
  test(`${provider.name} keeps explicit legacy publication and prepared target bytes`, (t) => {
    const { result, calls, files } = execute(t, provider, "pat");
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(
      calls.map(({ args }) => args.find((arg) => arg.endsWith(".vsix"))),
      files.map((file) => `vscode-targets/${file}`),
    );
    assert.ok(calls.every(({ args, patPresent }) => patPresent && !args.includes(provider.flag)));
  });

  test(`${provider.name} explicitly selects OIDC without an inherited legacy PAT`, (t) => {
    const { result, calls, files } = execute(t, provider, "oidc");
    assert.equal(result.status, 0, result.stderr);
    assert.equal(calls.length, files.length);
    assert.ok(calls.every(({ args, patPresent }) => args.includes(provider.flag) && !patPresent));
    const condition =
      provider.secret === "VSCE_PAT"
        ? "(vars.VSCE_AUTH_MODE == '' || vars.VSCE_AUTH_MODE == 'pat')"
        : "vars.OVSX_AUTH_MODE != 'oidc'";
    assert.ok(
      publishStep(provider.name).includes(
        `${provider.secret}: \${{ ${condition} && secrets.${provider.secret} || '' }}`,
      ),
    );
  });

  for (const [mode, identity, pat] of [
    ["invalid", true, true],
    ["oidc", false, true],
    ["pat", true, false],
  ]) {
    test(`${provider.name} rejects incomplete ${mode} authorization before upload`, (t) => {
      const { result, calls } = execute(t, provider, mode, identity, pat);
      assert.notEqual(result.status, 0);
      assert.match(result.stdout, /::error::/u);
      assert.deepEqual(calls, []);
    });
  }
}

test("Marketplace publishes prepared targets through Azure identity without legacy PAT", (t) => {
  const provider = providers[0];
  const { result, calls, files } = execute(t, provider, "azure", true, true, {
    AZURE_CLIENT_ID: "11111111-2222-3333-4444-555555555555",
    AZURE_TENANT_ID: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    MARKETPLACE_AZURE_AUTHENTICATED: "true",
    AZURE_CLIENT_SECRET: "synthetic-unwanted-secret",
    AZURE_CLIENT_CERTIFICATE_PATH: "synthetic-unwanted-certificate",
    AZURE_CLIENT_CERTIFICATE_PASSWORD: "synthetic-unwanted-password",
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.deepEqual(
    calls.map(({ args }) => args.find((arg) => arg.endsWith(".vsix"))),
    files.map((file) => `vscode-targets/${file}`),
  );
  assert.ok(
    calls.every(
      ({ args, patPresent, azureSecretPresent }) =>
        args.includes("--azure-credential") &&
        !args.includes("--oidc") &&
        !patPresent &&
        !azureSecretPresent,
    ),
  );
});

test("Marketplace Azure uploads require a complete successful configured identity", (t) => {
  const provider = providers[0];
  const ready = {
    AZURE_CLIENT_ID: "11111111-2222-3333-4444-555555555555",
    AZURE_TENANT_ID: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    MARKETPLACE_AZURE_AUTHENTICATED: "true",
  };
  const good = execute(t, provider, "azure", true, true, ready);
  assert.equal(good.result.status, 0, good.result.stdout + good.result.stderr);
  assert.deepEqual(
    good.calls.map(({ args }) => args.at(-3)),
    good.files.map((file) => `vscode-targets/${file}`),
  );
  for (const override of [
    { MARKETPLACE_AZURE_AUTHENTICATED: "false" },
    { AZURE_CLIENT_ID: "" },
    { AZURE_TENANT_ID: "" },
    { AZURE_CLIENT_ID: "not-a-client-id" },
    { AZURE_TENANT_ID: "not-a-tenant-id" },
    { ACTIONS_ID_TOKEN_REQUEST_TOKEN: "" },
  ]) {
    const denied = execute(t, provider, "azure", true, true, { ...ready, ...override });
    assert.notEqual(
      denied.result.status,
      0,
      `Accepted incomplete Azure identity: ${JSON.stringify(override)}`,
    );
    assert.match(denied.result.stdout, /::error::/u);
    assert.deepEqual(denied.calls, []);
  }
});

const azureConfigStep = () => {
  const start = workflow.indexOf(
    "      - name: Validate Marketplace Azure identity configuration\n",
  );
  assert.notEqual(start, -1, "Azure configuration must be validated before login");
  const end = workflow.indexOf("\n      - name:", start + 1);
  return workflow.slice(start, end);
};

test("Marketplace validates Azure configuration before requesting login", () => {
  const step = azureConfigStep();
  const block = step.match(/        run: \|\n([\s\S]+)/u);
  assert.ok(block);
  const script = block[1]
    .split("\n")
    .map((line) => line.slice(10))
    .join("\n");
  const ready = {
    PATH: "/usr/bin:/bin",
    GITHUB_ACTIONS: "true",
    AZURE_CLIENT_ID: "11111111-2222-3333-4444-555555555555",
    AZURE_TENANT_ID: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    ACTIONS_ID_TOKEN_REQUEST_URL: "http://127.0.0.1/synthetic",
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: "synthetic",
  };
  const invoke = (overrides = {}) =>
    spawnSync("bash", ["-euo", "pipefail", "-c", script], {
      encoding: "utf8",
      env: { ...ready, ...overrides },
    });
  const good = invoke();
  assert.equal(good.status, 0, good.stdout + good.stderr);
  for (const overrides of [
    { AZURE_CLIENT_ID: "" },
    { AZURE_TENANT_ID: "invalid" },
    { GITHUB_ACTIONS: "false" },
    { ACTIONS_ID_TOKEN_REQUEST_URL: "" },
  ]) {
    const denied = invoke(overrides);
    assert.notEqual(denied.status, 0);
    assert.match(denied.stdout, /::error::/u);
  }
});
