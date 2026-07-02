# VHS tape for the README demo GIF. Rendered in Docker (VHS hangs on Windows):
#   docker build -f docs/vhs.Dockerfile -t puppetty-vhs .
#   docker run --rm -v "$PWD:/vhs" puppetty-vhs docs/demo.tape
Output docs/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 900
Set Height 480
Set Theme "Catppuccin Mocha"
Set Padding 12
Set TypingSpeed 40ms

Type "puppetty run --auto -- node demo/prompt-demo.js"
Sleep 500ms
Enter

# The demo script asks three prompts; the autopilot answers each one.
Sleep 14s
