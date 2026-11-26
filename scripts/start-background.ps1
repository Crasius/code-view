# Starts a GUI app without taking the owner's foreground window (ship step, rule `rust`).
# Start-Process activates the new window; CreateProcess with STARTUPINFO.wShowWindow = SW_SHOWNOACTIVATE (4)
# does not. Use 7 (SW_SHOWMINNOACTIVE) to start minimized. Prints whether focus moved to the new process.
# The app gets the user's own environment (CreateEnvironmentBlock, as Explorer starts apps), not the
# caller's: an agent's shell carries NO_COLOR, session markers and tokens that would reach the app
# and every terminal and CLI it opens.
param([Parameter(Mandatory)][string]$Exe, [int]$Show = 4)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class BgStart {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct SI { public int cb; public string r; public string d; public string t; public int x,y,xs,ys,xc,yc,fa,flags; public short show; public short r2; public IntPtr r3,i,o,e; }
  [StructLayout(LayoutKind.Sequential)]
  public struct PI { public IntPtr hp, ht; public int pid, tid; }
  const int TOKEN_QUERY = 0x0008, TOKEN_DUPLICATE = 0x0002, CREATE_UNICODE_ENVIRONMENT = 0x0400;
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern bool CreateProcessW(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, int flags, IntPtr env, string dir, ref SI si, out PI pi);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr process, int access, out IntPtr token);
  [DllImport("userenv.dll", SetLastError=true)] static extern bool CreateEnvironmentBlock(out IntPtr env, IntPtr token, bool inherit);
  [DllImport("userenv.dll")] static extern bool DestroyEnvironmentBlock(IntPtr env);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid);
  public static int Start(string exe, string dir, short show) {
    IntPtr token, env;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, out token)) throw new System.ComponentModel.Win32Exception();
    try {
      if (!CreateEnvironmentBlock(out env, token, false)) throw new System.ComponentModel.Win32Exception();
    } finally { CloseHandle(token); }
    try {
      var si = new SI(); si.cb = Marshal.SizeOf(si); si.flags = 1; si.show = show;
      PI pi;
      if (!CreateProcessW(exe, "\"" + exe + "\"", IntPtr.Zero, IntPtr.Zero, false, CREATE_UNICODE_ENVIRONMENT, env, dir, ref si, out pi)) throw new System.ComponentModel.Win32Exception();
      CloseHandle(pi.hp); CloseHandle(pi.ht);
      return pi.pid;
    } finally { DestroyEnvironmentBlock(env); }
  }
  public static int ForegroundPid() { int p; GetWindowThreadProcessId(GetForegroundWindow(), out p); return p; }
}
"@
$full = (Resolve-Path $Exe).Path
$before = [BgStart]::ForegroundPid()
$started = [BgStart]::Start($full, (Split-Path $full), [int16]$Show)
Start-Sleep 4
$after = [BgStart]::ForegroundPid()
"started=$started foreground_before=$before foreground_after=$after stole_focus=$($after -eq $started)"
