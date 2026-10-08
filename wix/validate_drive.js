// PoolForge WiX Installer - Drive Letter Validator and Configurator

function InitDriveLetter() {
    try {
        var current = Session.Property("POOL_DRIVE_LETTER");
        var fso = new ActiveXObject("Scripting.FileSystemObject");

        if (!current || current === "") {
            // Check if V: is available by default
            if (!fso.DriveExists("V")) {
                Session.Property("POOL_DRIVE_LETTER") = "V:";
                Session.Property("DRIVE_VALID") = "1";
                return 1;
            }

            // Search for first available letter from Z down to D
            var candidates = ["Z", "Y", "X", "W", "U", "T", "S", "R", "Q", "P", "O", "N", "M", "L", "K", "J", "I", "H", "G", "F", "E", "D"];
            for (var i = 0; i < candidates.length; i++) {
                if (!fso.DriveExists(candidates[i])) {
                    Session.Property("POOL_DRIVE_LETTER") = candidates[i] + ":";
                    Session.Property("DRIVE_VALID") = "1";
                    return 1;
                }
            }
            Session.Property("POOL_DRIVE_LETTER") = "V:";
        }
        Session.Property("DRIVE_VALID") = "1";
    } catch (e) {
        // Fallback safely
        Session.Property("POOL_DRIVE_LETTER") = "V:";
        Session.Property("DRIVE_VALID") = "1";
    }
    return 1;
}

function ValidateDriveLetter() {
    try {
        var raw = Session.Property("POOL_DRIVE_LETTER");
        if (!raw || raw.replace(/\s+/g, '') === "") {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Please enter a drive letter (e.g. V:).";
            return 1;
        }

        var letter = raw.replace(/\s+/g, '').toUpperCase();
        if (letter.length === 1) {
            letter = letter + ":";
        }

        if (letter.length !== 2 || letter.charAt(1) !== ':') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Invalid format '" + raw + "'. Please specify a single drive letter followed by a colon (e.g. V:).";
            return 1;
        }

        var ch = letter.charAt(0);
        if (ch < 'A' || ch > 'Z') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "'" + ch + "' is not a valid drive letter. Please choose between A: and Z:.";
            return 1;
        }

        if (ch === 'C') {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Drive letter C: is reserved for the Windows operating system and cannot be used for the storage pool.";
            return 1;
        }

        var fso = new ActiveXObject("Scripting.FileSystemObject");
        if (fso.DriveExists(ch)) {
            Session.Property("DRIVE_VALID") = "0";
            Session.Property("DRIVE_ERROR_MSG") = "Drive letter " + letter + " is already in use by an existing drive, volume, optical disc, or network share. Please select an available drive letter.";
            return 1;
        }

        // Letter is valid and free
        Session.Property("POOL_DRIVE_LETTER") = letter;
        Session.Property("DRIVE_VALID") = "1";
        Session.Property("DRIVE_ERROR_MSG") = "";
    } catch (err) {
        Session.Property("DRIVE_VALID") = "0";
        Session.Property("DRIVE_ERROR_MSG") = "Verification failed: " + err.message;
    }
    return 1;
}

function ApplyConfigDriveLetter() {
    try {
        var installFolder = "";
        var targetLetter = "V:";

        var cad = Session.Property("CustomActionData");
        if (cad && cad !== "") {
            var parts = cad.split("|");
            installFolder = parts[0];
            if (parts.length > 1 && parts[1] !== "") {
                targetLetter = parts[1];
            }
        }

        if (!installFolder || installFolder === "") {
            installFolder = Session.Property("INSTALLFOLDER");
        }
        if (!targetLetter || targetLetter === "V:") {
            var propLetter = Session.Property("POOL_DRIVE_LETTER");
            if (propLetter && propLetter !== "") {
                targetLetter = propLetter;
            }
        }

        if (!installFolder || installFolder === "") {
            return 1;
        }

        var fso = new ActiveXObject("Scripting.FileSystemObject");
        var configFile = fso.BuildPath(installFolder, "config.json");

        if (fso.FileExists(configFile)) {
            var tsIn = fso.OpenTextFile(configFile, 1, false); // ForReading
            var content = tsIn.ReadAll();
            tsIn.Close();

            // Replace "mount_point": "..."
            var regex = /"mount_point"\s*:\s*"[^"]*"/;
            if (regex.test(content)) {
                var newContent = content.replace(regex, '"mount_point": "' + targetLetter + '"');
                var tsOut = fso.OpenTextFile(configFile, 2, true); // ForWriting
                tsOut.Write(newContent);
                tsOut.Close();
            }
        }
    } catch (e) {
        // Do not block install if file write encounters non-fatal issue
    }
    return 1;
}

function InstallWinFsp() {
    try {
        var shell = new ActiveXObject("WScript.Shell");
        var fso = new ActiveXObject("Scripting.FileSystemObject");
        var tempFolder = shell.ExpandEnvironmentStrings("%TEMP%");
        var ps1File = fso.BuildPath(tempFolder, "install_winfsp.ps1");

        var ts = fso.OpenTextFile(ps1File, 2, true); // ForWriting
        ts.WriteLine("$ErrorActionPreference = 'Stop'");
        ts.WriteLine("try {");
        ts.WriteLine("    Write-Host '==================================================' -ForegroundColor Cyan");
        ts.WriteLine("    Write-Host '   PoolForge Prerequisite: Installing WinFsp' -ForegroundColor Cyan");
        ts.WriteLine("    Write-Host '==================================================' -ForegroundColor Cyan");
        ts.WriteLine("    $url = 'https://github.com/winfsp/winfsp/releases/download/v2.0/winfsp-2.0.23075.msi'");
        ts.WriteLine("    $dest = Join-Path $env:TEMP 'winfsp-installer.msi'");
        ts.WriteLine("    Write-Host 'Downloading WinFsp 2.0 package from GitHub...' -ForegroundColor Yellow");
        ts.WriteLine("    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12");
        ts.WriteLine("    (New-Object System.Net.WebClient).DownloadFile($url, $dest)");
        ts.WriteLine("    Write-Host 'Download complete. Installing WinFsp silently...' -ForegroundColor Green");
        ts.WriteLine("    $proc = Start-Process msiexec.exe -ArgumentList ('/i \"' + $dest + '\" /passive /norestart') -Wait -PassThru");
        ts.WriteLine("    if ($proc.ExitCode -eq 0 -or $proc.ExitCode -eq 3010) {");
        ts.WriteLine("        Write-Host 'WinFsp installed successfully!' -ForegroundColor Green");
        ts.WriteLine("        Start-Sleep -Seconds 2");
        ts.WriteLine("    } else {");
        ts.WriteLine("        Write-Host ('WinFsp installation returned code ' + $proc.ExitCode) -ForegroundColor Red");
        ts.WriteLine("        Write-Host 'Press Enter to continue...' -ForegroundColor Yellow");
        ts.WriteLine("        Read-Host");
        ts.WriteLine("    }");
        ts.WriteLine("} catch {");
        ts.WriteLine("    Write-Host ('Error: ' + $_.Exception.Message) -ForegroundColor Red");
        ts.WriteLine("    Write-Host 'Press Enter to continue...' -ForegroundColor Yellow");
        ts.WriteLine("    Read-Host");
        ts.WriteLine("} finally {");
        ts.WriteLine("    Remove-Item (Join-Path $env:TEMP 'winfsp-installer.msi') -Force -ErrorAction SilentlyContinue");
        ts.WriteLine("}");
        ts.Close();

        var cmd = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File \"" + ps1File + "\"";
        shell.Run(cmd, 1, true);

        try { fso.DeleteFile(ps1File); } catch (e) {}

        CheckWinFspStatus();
    } catch (e) {
        // Fallback
    }
    return 1;
}

function OpenWinFspUrl() {
    try {
        var shell = new ActiveXObject("WScript.Shell");
        shell.Run("https://github.com/winfsp/winfsp/releases");
    } catch (e) {}
    return 1;
}

function CheckWinFspStatus() {
    try {
        var fso = new ActiveXObject("Scripting.FileSystemObject");
        var shell = new ActiveXObject("WScript.Shell");
        var installed = false;

        var p1 = "C:\\Program Files (x86)\\WinFsp\\bin\\winfsp-x64.dll";
        var p2 = "C:\\Program Files\\WinFsp\\bin\\winfsp-x64.dll";
        if (fso.FileExists(p1) || fso.FileExists(p2)) {
            installed = true;
        }

        try {
            var reg = shell.RegRead("HKLM\\SOFTWARE\\WinFsp\\InstallDir");
            if (reg && reg !== "") installed = true;
        } catch(e) {}

        try {
            var regWow = shell.RegRead("HKLM\\SOFTWARE\\WOW6432Node\\WinFsp\\InstallDir");
            if (regWow && regWow !== "") installed = true;
        } catch(e) {}

        if (installed) {
            Session.Property("WINFSP_INSTALLED") = "1";
            Session.Property("WINFSP_STATUS_TEXT") = "WinFsp is installed and ready.";
        } else {
            Session.Property("WINFSP_STATUS_TEXT") = "WinFsp is not detected. Please install it before proceeding.";
        }
    } catch (e) {}
    return 1;
}

