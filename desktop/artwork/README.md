# Windows-MCP icon

The cobalt window and cyan pointer represent desktop control. The mark uses broad strokes to remain readable in the Windows tray.

- `windows-mcp-icon.png`: original RGBA master generated with the built-in ImageGen tool.
- `windows-mcp-icon.prompt.txt`: final generation prompt.
- `../src-tauri/icons/`: exported Windows PNG/ICO assets and ICNS.

Regenerate the platform assets with the Tauri CLI:

```powershell
npm run tauri -- icon artwork/windows-mcp-icon.png --output <export-directory>
```

Copy the root PNG, ICO and ICNS files into `src-tauri/icons/`, and copy `128x128.png` to `public/app-icon.png`. The manager and file-transfer page import the same exported PNG. Tauri uses the icon for the app executable, window and native tray. NSIS explicitly uses `icons/icon.ico` for both the installer and uninstaller; shortcuts use the application executable's icon.
