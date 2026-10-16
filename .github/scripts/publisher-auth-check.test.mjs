import assert from "node:assert/strict";
import { test } from "node:test";
import { verifyOpenVsxIdentity } from "./publisher-auth-check.mjs";

const env = {
  GITHUB_ACTIONS: "true",
  GITHUB_REF: "refs/heads/main",
  GITHUB_WORKFLOW_REF: "fallow-rs/fallow/.github/workflows/release.yml@refs/heads/main",
  ACTIONS_ID_TOKEN_REQUEST_URL:
    "https://vstoken.actions.githubusercontent.com/token?api-version=2.0",
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: "synthetic-request",
};

test("Open VSX checks fixed identity exchange and namespace permission without packages or PAT", async () => {
  const calls = [];
  const send = async (url, options) => {
    calls.push({ url: String(url), ...options });
    const payloads = [
      { value: "synthetic-identity" },
      { value: "synthetic-authority" },
      { success: "Valid token" },
    ];
    return Response.json(payloads[calls.length - 1]);
  };
  assert.deepEqual(await verifyOpenVsxIdentity({ ...env, OVSX_PAT: "must-not-be-used" }, send), {
    ok: true,
  });
  assert.equal(calls.length, 3);
  assert.equal(new URL(calls[0].url).searchParams.get("audience"), "https://open-vsx.org");
  assert.equal(calls[0].headers.Authorization, "Bearer synthetic-request");
  assert.equal(calls[1].url, "https://open-vsx.org/api/-/trusted-publishing/token");
  assert.equal(calls[1].method, "POST");
  assert.deepEqual(JSON.parse(calls[1].body), {
    namespace: "fallow-rs",
    extension: "fallow-vscode",
    token: "synthetic-identity",
  });
  assert.equal(calls[2].url, "https://open-vsx.org/api/fallow-rs/verify-pat");
  assert.equal(calls[2].method, "GET");
  assert.equal(calls[2].headers.Authorization, "Bearer synthetic-authority");
  for (const call of calls) assert.equal(call.redirect, "error");
});

test("Open VSX refuses identity and permission failures without token fallback or publication", async () => {
  for (const failedRequest of [0, 1, 2]) {
    let calls = 0;
    const send = async () => {
      const index = calls++;
      if (index === failedRequest) return new Response("private provider details", { status: 403 });
      return Response.json({ value: "synthetic-token" });
    };
    const result = await verifyOpenVsxIdentity({ ...env, OVSX_PAT: "must-not-be-used" }, send);
    assert.equal(result.ok, false);
    assert.equal(calls, failedRequest + 1);
    assert.doesNotMatch(result.error, /synthetic|private provider/u);
  }
  let calls = 0;
  const never = async () => {
    calls++;
    throw new Error("Unexpected network");
  };
  for (const override of [
    { GITHUB_REF: "refs/heads/other" },
    { GITHUB_WORKFLOW_REF: "other/workflow" },
    { ACTIONS_ID_TOKEN_REQUEST_URL: "https://attacker.example/token" },
    {
      ACTIONS_ID_TOKEN_REQUEST_URL:
        "https://vstoken.actions.githubusercontent.com.attacker.example/token",
    },
    { ACTIONS_ID_TOKEN_REQUEST_URL: "http://vstoken.actions.githubusercontent.com/token" },
    { ACTIONS_ID_TOKEN_REQUEST_TOKEN: "" },
  ])
    assert.equal((await verifyOpenVsxIdentity({ ...env, ...override }, never)).ok, false);
  assert.equal(calls, 0);
  assert.deepEqual(
    await verifyOpenVsxIdentity(env, async () =>
      Response.json({ value: "token", success: "Valid token" }),
    ),
    { ok: true },
  );
});

test("Open VSX check exercises HTTP exchange and permission reads on loopback", async (t) => {
  const { createServer } = await import("node:http");
  let requests = 0;
  const server = createServer(async (request, response) => {
    requests++;
    response.setHeader("Content-Type", "application/json");
    if (request.url.startsWith("/token")) {
      assert.equal(request.headers.authorization, "Bearer synthetic-request");
      response.end(JSON.stringify({ value: "synthetic-identity" }));
      return;
    }
    if (request.url === "/api/-/trusted-publishing/token") {
      assert.equal(request.method, "POST");
      let body = "";
      for await (const chunk of request) body += chunk;
      assert.deepEqual(JSON.parse(body), {
        namespace: "fallow-rs",
        extension: "fallow-vscode",
        token: "synthetic-identity",
      });
      response.end(JSON.stringify({ value: "synthetic-authority" }));
      return;
    }
    assert.equal(request.url, "/api/fallow-rs/verify-pat");
    assert.equal(request.method, "GET");
    assert.equal(request.headers.authorization, "Bearer synthetic-authority");
    response.end(JSON.stringify({ success: "Valid token" }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  });
  const send = (url, options) => {
    const fixed = new URL(url);
    assert.ok(["vstoken.actions.githubusercontent.com", "open-vsx.org"].includes(fixed.hostname));
    return fetch(
      `http://127.0.0.1:${server.address().port}${fixed.pathname}${fixed.search}`,
      options,
    );
  };
  assert.deepEqual(await verifyOpenVsxIdentity(env, send), { ok: true });
  assert.equal(requests, 3);
});
