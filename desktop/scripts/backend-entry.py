"""Frozen executable entry point; all MCP behavior stays in the official package."""
from multiprocessing import freeze_support

if __name__ == "__main__":
    freeze_support()
    from windows_mcp.__main__ import main

    main()
