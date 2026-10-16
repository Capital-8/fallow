import { fileURLToPath } from "node:url";

const REGISTRY = "https://open-vsx.org";
const REQUEST_TIMEOUT_MS = 10_000;
const WORKFLOW_REF = "fallow-rs/fallow/.github/workflows/release.yml@refs/heads/main";

const requestJson = async (url, options, send) => {
  const response = await send(url, {
    ...options,
    redirect: "error",
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  if (response.status !== 200) throw new Error("Identity request was refused");
  const value = await response.json();
  if (!value || typeof value !== "object" || value.error) {
    throw new Error("Identity response was invalid");
  }
  return value;
};

const validToken = (value) =>
  typeof value === "string" && value.length > 0 && !/[\r\n]/u.test(value);

/** Check the fixed extension's trusted identity and namespace rights without uploading packages. */
export const verifyOpenVsxIdentity = async (env = process.env, send = fetch) => {
  try {
    if (
      env.GITHUB_ACTIONS !== "true" ||
      env.GITHUB_REF !== "refs/heads/main" ||
      env.GITHUB_WORKFLOW_REF !== WORKFLOW_REF ||
      !validToken(env.ACTIONS_ID_TOKEN_REQUEST_TOKEN)
    ) {
      return {
        ok: false,
        error: "Open VSX identity check requires the protected release workflow",
      };
    }
    const identityUrl = new URL(env.ACTIONS_ID_TOKEN_REQUEST_URL);
    if (
      identityUrl.protocol !== "https:" ||
      !identityUrl.hostname.endsWith(".actions.githubusercontent.com") ||
      identityUrl.username ||
      identityUrl.password ||
      identityUrl.port
    ) {
      return { ok: false, error: "GitHub identity endpoint is invalid" };
    }
    identityUrl.searchParams.set("audience", REGISTRY);
    const identity = await requestJson(
      identityUrl,
      {
        method: "GET",
        headers: {
          Authorization: `Bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}`,
          Accept: "application/json",
        },
      },
      send,
    );
    if (!validToken(identity.value)) throw new Error("Missing identity");
    const authority = await requestJson(
      `${REGISTRY}/api/-/trusted-publishing/token`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({
          namespace: "fallow-rs",
          extension: "fallow-vscode",
          token: identity.value,
        }),
      },
      send,
    );
    if (!validToken(authority.value)) throw new Error("Missing authority");
    const permission = await requestJson(
      `${REGISTRY}/api/fallow-rs/verify-pat`,
      {
        method: "GET",
        headers: { Authorization: `Bearer ${authority.value}`, Accept: "application/json" },
      },
      send,
    );
    if (typeof permission.success !== "string" || !permission.success)
      throw new Error("Missing permission");
    return { ok: true };
  } catch {
    return { ok: false, error: "Open VSX trusted identity or namespace permission check failed" };
  }
};

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const result =
    process.argv.length === 2
      ? await verifyOpenVsxIdentity()
      : { ok: false, error: "Unsupported identity check arguments" };
  console.log(
    result.ok ? "Open VSX trusted identity has namespace publishing permission" : result.error,
  );
  process.exitCode = result.ok ? 0 : 1;
}
