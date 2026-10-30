@echo off
rem Relay mode: phone page on Vercel, signaling via Cloudflare. No certificate setup needed.
call "%~dp0run.bat" --relay wss://phonecam-relay.phonecam-relay.workers.dev --page https://phonecam-eta.vercel.app/ %*
