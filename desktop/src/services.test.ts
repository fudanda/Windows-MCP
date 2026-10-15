import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, preview, type Snapshot } from "./api";
import { startServices, stopServices } from "./services";
import { transferApi, type TransferSnapshot } from "./transfer-api";

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return {
    ...actual,
    api: { ...actual.api, save: vi.fn(), start: vi.fn(), stop: vi.fn() },
  };
});

describe("combined service shutdown", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.stop).mockResolvedValue(stopped);
    vi.mocked(transferApi.command).mockResolvedValue({ ...fileService, running: false });
  });

  it("stops file transfer even while MCP shutdown is pending", async () => {
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.stop).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const update = vi.fn();
    const shutdown = stopServices("zh-CN", update);
    expect(transferApi.command).toHaveBeenCalledWith("stop");
    finish(stopped);
    await shutdown;
    expect(update).toHaveBeenCalledWith(stopped);
  });

  it.each(["mcp", "transfer"])("still stops the other service when %s shutdown fails", async (failed) => {
    if (failed === "mcp") vi.mocked(api.stop).mockRejectedValue({ code: "process_failed" });
    else vi.mocked(transferApi.command).mockRejectedValue({ code: "transfer_io" });
    const update = vi.fn();
    await expect(stopServices("en", update)).rejects.toMatchObject({
      code: "services_stop_failed",
      detail: expect.stringContaining(failed === "mcp" ? "MCP:" : "File transfer:"),
    });
    expect(api.stop).toHaveBeenCalledTimes(1);
    expect(transferApi.command).toHaveBeenCalledWith("stop");
    if (failed === "transfer") expect(update).toHaveBeenCalledWith(stopped);
  });

  it("cancels a pending MCP startup before file transfer can start", async () => {
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.start).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const controller = new AbortController();
    const promise = startServices(stopped, stopped.config, vi.fn(), controller.signal);
    const shutdown = stopServices("zh-CN", vi.fn(), { controller, promise });
    expect(controller.signal.aborted).toBe(true);
    finish(stopped);
    await shutdown;
    expect(transferApi.command).toHaveBeenCalledExactlyOnceWith("stop");
  });

  it("drains a queued file-service start before the final stop", async () => {
    let finish!: (value: TransferSnapshot) => void;
    const starting = new Promise<TransferSnapshot>((resolve) => { finish = resolve; });
    vi.mocked(transferApi.command).mockImplementation(async (action) =>
      action === "start" ? starting : { ...fileService, running: false },
    );
    const controller = new AbortController();
    const promise = startServices(running, running.config, vi.fn(), controller.signal);
    const shutdown = stopServices("zh-CN", vi.fn(), { controller, promise });
    expect(transferApi.command).not.toHaveBeenCalledWith("stop");
    finish(fileService);
    await shutdown;
    expect(vi.mocked(transferApi.command).mock.calls.map(([action]) => action)).toEqual(["start", "stop"]);
  });

  it.each(["mcp", "transfer"])("reports failure if %s is still running after its stop response", async (active) => {
    if (active === "mcp") vi.mocked(api.stop).mockResolvedValue(running);
    else vi.mocked(transferApi.command).mockResolvedValue(fileService);
    await expect(stopServices("zh-CN", vi.fn())).rejects.toMatchObject({ code: "services_stop_failed" });
  });
});
vi.mock("./transfer-api", () => ({
  transferApi: { command: vi.fn() },
}));

const stopped: Snapshot = {
  ...preview,
  generation: 4,
  backendAvailable: true,
  config: { ...preview.config, token: "a".repeat(32) },
};
const running: Snapshot = { ...stopped, phase: "running", generation: 5 };
const fileService: TransferSnapshot = {
  running: true,
  settings: {
    port: 8002,
    lanEnabled: true,
    sharedDir: "D:\\Receive",
    token: "b".repeat(32),
    peers: [],
  },
  tasks: [],
  engine: "Dufs 0.46.0",
};

describe("combined service startup", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.start).mockResolvedValue(running);
    vi.mocked(transferApi.command).mockResolvedValue(fileService);
  });

  it("saves pending MCP settings and waits for MCP readiness before starting file transfer", async () => {
    const config = { ...stopped.config, port: 8101 };
    const saved = { ...stopped, config, generation: 5 };
    const ready = { ...running, config, generation: 6 };
    vi.mocked(api.save).mockResolvedValue(saved);
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.start).mockReturnValue(new Promise((resolve) => {
      finish = resolve;
    }));
    const update = vi.fn();
    const start = startServices(stopped, config, update);
    await vi.waitFor(() => expect(api.start).toHaveBeenCalledWith(5));
    expect(api.save).toHaveBeenCalledWith(config);
    expect(transferApi.command).not.toHaveBeenCalled();
    finish(ready);
    await start;
    expect(update.mock.calls.map(([value]) => value)).toEqual([saved, ready]);
    expect(transferApi.command).toHaveBeenCalledWith("start");
  });

  it("reuses a running MCP service, including after a partial failure", async () => {
    const update = vi.fn();
    await startServices(running, running.config, update);
    expect(api.save).not.toHaveBeenCalled();
    expect(api.start).not.toHaveBeenCalled();
    expect(update).not.toHaveBeenCalled();
    expect(transferApi.command).toHaveBeenCalledExactlyOnceWith("start");
  });

  it("does not enqueue MCP startup if cancelled while saving configuration", async () => {
    const config = { ...stopped.config, port: 8101 };
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.save).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const controller = new AbortController();
    const promise = startServices(stopped, config, vi.fn(), controller.signal);
    const rejected = expect(promise).rejects.toMatchObject({ name: "AbortError" });
    controller.abort();
    finish({ ...stopped, config, generation: 5 });
    await rejected;
    expect(api.start).not.toHaveBeenCalled();
    expect(transferApi.command).not.toHaveBeenCalled();
  });

  it.each(["error", "stopped"] as const)("does not start file transfer when MCP returns %s", async (phase) => {
    const result = { ...stopped, phase, error: phase === "error" ? { code: "startup_timeout", detail: "timeout" } : null };
    vi.mocked(api.start).mockResolvedValue(result);
    const update = vi.fn();
    await expect(startServices(stopped, stopped.config, update)).rejects.toMatchObject({
      code: phase === "error" ? "startup_timeout" : "not_running",
    });
    expect(update).toHaveBeenCalledWith(result);
    expect(transferApi.command).not.toHaveBeenCalled();
  });

  it("preserves MCP on transfer failure and gives a localized reason for retry", async () => {
    vi.mocked(transferApi.command).mockRejectedValue({ code: "port_in_use", detail: "address in use" });
    const update = vi.fn();
    await expect(startServices(stopped, stopped.config, update)).rejects.toMatchObject({
      code: "transfer_start_failed",
      detail: expect.stringContaining("端口"),
    });
    expect(update).toHaveBeenLastCalledWith(running);
    vi.mocked(transferApi.command).mockResolvedValue(fileService);
    await startServices(running, running.config, update);
    expect(api.start).toHaveBeenCalledTimes(1);
  });

  it("rejects an unsuccessful file service response instead of announcing success", async () => {
    vi.mocked(transferApi.command).mockResolvedValue({ ...fileService, running: false });
    await expect(startServices(running, { ...running.config, language: "en" }, vi.fn())).rejects.toEqual({
      code: "transfer_start_failed",
      detail: "Start the file service first.",
    });
  });
});
