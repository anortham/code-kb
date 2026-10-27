function AnchorPowershell([int]$Value) { return $Value + 41 }
function CallerPowershell() { AnchorPowershell 1 }
