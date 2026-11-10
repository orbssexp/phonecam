param([int]$ProcId, [int]$Secs = 10)
$a = (Get-Process -Id $ProcId).CPU; Start-Sleep $Secs; $p = Get-Process -Id $ProcId
$cores = [Environment]::ProcessorCount
"cpu: {0}% of one core, {1}% of whole CPU ({2} threads) | working set {3} MB | private {4} MB" -f [math]::Round(($p.CPU-$a)/$Secs*100,1), [math]::Round(($p.CPU-$a)/$Secs*100/$cores,2), $cores, [int]($p.WorkingSet64/1MB), [int]($p.PrivateMemorySize64/1MB)
