<?php
function anchor_php(int $value): int { return $value + 41; }
function caller_php(): int { return anchor_php(1); }
