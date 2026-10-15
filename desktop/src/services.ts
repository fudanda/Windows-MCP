import { api, connectionChanged, type Config, type Snapshot } from "./api";
import { errorMessage, type Language } from "./i18n";
import { transferApi } from "./transfer-api";
import { transferError } from "./transfer-i18n";

export interface ServiceStartup {
  controller: AbortController;
  promise: Promise<void>;
}

function serviceError(language: Language, error: unknown): string {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? String(error.code)
      : "";
  return code.startsWith("transfer_")
    ? transferError(language, error)
    : errorMessage(language, error);
}

export async function startServices(
  snapshot: Snapshot,
  config: Config,
  update: (snapshot: Snapshot) => void,
  signal?: AbortSignal,
): Promise<void> {
  signal?.throwIfAborted();
  let current = snapshot;
  if (["starting", "stopping"].includes(current.phase))
    throw { code: "already_running" };
  if (current.phase !== "running") {
    if (connectionChanged(config, current.config)) {
      current = await api.save(config);
      update(current);
    }
    signal?.throwIfAborted();
    current = await api.start(current.generation);
    update(current);
    signal?.throwIfAborted();
    // A cancelled or failed startup can return a snapshot without rejecting.
    if (current.phase !== "running")
      throw current.error ?? { code: "not_running" };
  }
  signal?.throwIfAborted();
  try {
    const transfer = await transferApi.command("start");
    if (!transfer.running) throw { code: "transfer_not_running" };
  } catch (error) {
    throw {
      code: "transfer_start_failed",
      detail: serviceError(config.language, error),
    };
  }
  signal?.throwIfAborted();
}

export async function stopServices(
  language: Language,
  update: (snapshot: Snapshot) => void,
  pending?: ServiceStartup | null,
): Promise<void> {
  if (pending) {
    pending.controller.abort();
    // Interrupt MCP startup, then drain any queued start before the final stop.
    // The final calls below verify shutdown even if the interruption failed.
    await Promise.allSettled([api.stop(), pending.promise]);
  }
  const results = await Promise.allSettled([
    api.stop().then((snapshot) => {
      update(snapshot);
      if (snapshot.phase !== "stopped")
        throw snapshot.error ?? { code: "process_failed" };
    }),
    transferApi.command("stop").then((snapshot) => {
      if (snapshot.running) throw { code: "transfer_stop_failed" };
    }),
  ]);
  const labels = ["MCP", language === "en" ? "File transfer" : "文件传输"];
  const failures = results.flatMap((result, index) =>
    result.status === "rejected"
      ? [`${labels[index]}: ${serviceError(language, result.reason)}`]
      : [],
  );
  if (failures.length)
    throw { code: "services_stop_failed", detail: failures.join("\n") };
}
