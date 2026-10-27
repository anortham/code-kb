<template><button @click="caller_vue">{{ anchor_vue(1) }}</button></template>
<script>
export function anchor_vue(value) { return value + 41; }
export function caller_vue() { return anchor_vue(1); }
</script>
