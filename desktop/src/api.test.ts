import { describe, expect, it } from "vitest";
import {
  connectionChanged,
  connectionConfig,
  validConfig,
  type Config,
} from "./api";
import { errorMessage, translate } from "./i18n";

const config: Config = {
  language: "zh-CN",
  port: 8001,
  lanEnabled: true,
  token: "a".repeat(32),
};
describe("client configuration", () => {
  it("includes the LAN endpoint and bearer header only in the copied version", () => {
    const output = JSON.parse(connectionConfig(config, "192.168.18.4", "json"));
    expect(output.mcpServers["windows-mcp"].url).toBe(
      "http://192.168.18.4:8001/mcp",
    );
    expect(output.mcpServers["windows-mcp"].headers.Authorization).toBe(
      `Bearer ${config.token}`,
    );
    expect(connectionConfig(config, "127.0.0.1", "toml", true)).not.toContain(
      config.token,
    );
    expect(connectionConfig(config, "127.0.0.1", "toml")).toContain(
      "http_headers = { Authorization",
    );
  });
  it("does not treat a language change as an unapplied server change", () => {
    expect(connectionChanged(config, { ...config, language: "en" })).toBe(
      false,
    );
    expect(connectionChanged(config, { ...config, port: 8002 })).toBe(true);
    expect(
      connectionChanged(config, { ...config, token: "b".repeat(32) }),
    ).toBe(true);
  });
  it("rejects invalid ports and tokens before starting", () => {
    expect(validConfig(config)).toBe(true);
    for (const port of [0, -1, 65536, 2.5, NaN])
      expect(validConfig({ ...config, port })).toBe(false);
    for (const token of ["", "short", " ".repeat(32), "中文".repeat(32)])
      expect(validConfig({ ...config, token })).toBe(false);
  });
});
describe("interface language", () => {
  it("translates controls and native errors in both languages", () => {
    expect(translate("zh-CN", "start")).toBe("启动服务");
    expect(translate("en", "start")).toBe("Start service");
    expect(errorMessage("zh-CN", { code: "port_in_use" })).toContain("端口");
    expect(errorMessage("en", { code: "port_in_use" })).toContain("port");
  });
});
