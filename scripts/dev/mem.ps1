param([string]$Tag)
$q = Get-Process phonecam -EA SilentlyContinue | Select-Object -First 1
if ($q) { "{0}: ws {1} MB, private {2} MB, threads {3}, handles {4}" -f $Tag, [int]($q.WorkingSet64/1MB), [int]($q.PrivateMemorySize64/1MB), $q.Threads.Count, $q.HandleCount } else { "${Tag}: not running" }
