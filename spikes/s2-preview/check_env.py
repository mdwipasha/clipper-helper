import subprocess
import os
import sys
import winreg

def get_webview2_version():
    keys = [
        (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-F552-44DD-9A17-1B46CE810E4A}"),
        (winreg.HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-F552-44DD-9A17-1B46CE810E4A}"),
        (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-F552-44DD-9A17-1B46CE810E4A}"),
    ]
    for root, subkey in keys:
        try:
            with winreg.OpenKey(root, subkey) as key:
                val, _ = winreg.QueryValueEx(key, "pv")
                if val:
                    return val
        except FileNotFoundError:
            continue
        except Exception as e:
            pass
    
    # Try directory scan
    dirs = [
        r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
        r"C:\Program Files\Microsoft\EdgeWebView\Application",
        os.path.expandvars(r"%LOCALAPPDATA%\Microsoft\EdgeWebView\Application")
    ]
    for d in dirs:
        if os.path.exists(d):
            subdirs = [s for s in os.listdir(d) if s[0].isdigit() and os.path.isdir(os.path.join(d, s))]
            if subdirs:
                return sorted(subdirs)[-1]
    return "Unknown"

def get_systeminfo():
    try:
        res = subprocess.run(["systeminfo"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30)
        return res.stdout
    except Exception as e:
        return str(e)

if __name__ == "__main__":
    wv_ver = get_webview2_version()
    print(f"WebView2 Runtime Version: {wv_ver}")
    
    sysinfo = get_systeminfo()
    for line in sysinfo.splitlines()[:25]:
        print(line)
