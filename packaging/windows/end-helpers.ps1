# Run by the installer (swell.iss) during an update, when programs of the
# install still run 10 s after the Swell that started the update quit.
# Ends the ones that outlived it, so their files can be replaced, and says
# what it did on its output, which goes to the installer's log:
#
#   end-helpers.ps1 -AppDir <install folder> -SwellPid <the Swell that quit>
#
# Ended: GLib's gdbus.exe (the session bus GApplication starts), unless
# another program is connected to it; GLib's spawn helpers; GStreamer's
# plugin scanner; and the Swell that started the update, which by now only
# hangs on its way out. Any other program, another Swell included, is left
# running.
param(
    [Parameter(Mandatory = $true)][string] $AppDir,
    [int] $SwellPid = 0
)

$prefix = $AppDir.TrimEnd('\') + '\'
$helpers = @('gdbus.exe', 'gspawn-win64-helper.exe', 'gspawn-win64-helper-console.exe',
    'gst-plugin-scanner.exe')

# Win32_Process has the paths of 64-bit programs also for a 32-bit shell.
foreach ($process in @(Get-CimInstance Win32_Process)) {
    # Empty for the processes of other users.
    $path = $process.ExecutablePath
    if (-not $path -or -not $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        continue
    }
    $id = [int]$process.ProcessId
    $name = [IO.Path]::GetFileName($path)
    $what = "$path (process $id)"
    if ($name -eq 'swell-player.exe') {
        if ($id -ne $SwellPid) {
            Write-Output "Left running: $what, another Swell"
            continue
        }
    } elseif ($helpers -notcontains $name) {
        Write-Output "Left running: $what, not one of Swell's helpers"
        continue
    } elseif ($name -eq 'gdbus.exe') {
        # The bus listens on localhost (nonce-tcp). Any other program of the
        # session may have connected to it: ending it would end them too.
        $ports = @(Get-NetTCPConnection -OwningProcess $id -State Listen -ErrorAction SilentlyContinue |
            ForEach-Object { $_.LocalPort })
        if ($ports.Count -eq 0) {
            Write-Output "Left running: $what, whose connections could not be listed"
            continue
        }
        $clients = @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue |
            Where-Object {
                $ports -contains $_.RemotePort -and $_.OwningProcess -ne $id -and
                @('127.0.0.1', '::1') -contains $_.RemoteAddress
            })
        if ($clients.Count -gt 0) {
            $names = @($clients | ForEach-Object {
                    (Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue).Name
                } | Where-Object { $_ } | Sort-Object -Unique)
            Write-Output "Left running: $what, the session bus of $($names -join ', ')"
            continue
        }
    }
    Write-Output "Ending $what"
    Stop-Process -Id $id -Force -ErrorAction Continue
}
