Output assets/demo.gif

Require target/release/zz

Set Shell "bash"
Set FontFamily "JetBrains Mono"
Set FontSize 22
Set Width 1100
Set Height 620
Set LetterSpacing 0
Set LineHeight 1.2
Set Padding 28
Set Margin 12
Set MarginFill "#000000"
Set BorderRadius 10
Set WindowBar "RingsRight"
Set WindowBarSize 36
Set TypingSpeed 28ms
Set Framerate 30
Set PlaybackSpeed 1
Set LoopOffset 12%
Set Theme {"background":"#000000","foreground":"#f7f8f8","cursor":"#f7f8f8","selection":"#383838","black":"#000000","brightBlack":"#888888","red":"#d0d0d0","brightRed":"#ffffff","green":"#d0d0d0","brightGreen":"#ffffff","yellow":"#d0d0d0","brightYellow":"#ffffff","blue":"#d0d0d0","brightBlue":"#ffffff","magenta":"#d0d0d0","brightMagenta":"#ffffff","cyan":"#d0d0d0","brightCyan":"#ffffff","white":"#f7f8f8","brightWhite":"#ffffff"}

Type "clear"
Enter
Type "HOME=/tmp/zz-demo-home target/release/zz"
Enter
Sleep 1s

Type "i"
Type "Fix the draft recovery bug."
Enter
Type "Preserve file permissions and add a regression test for "
Type "@src/storage.rs:128-176"
Sleep 1.5s
Enter
Type "."
Sleep 1.5s

Escape
Type "ZZ"
Sleep 2s
