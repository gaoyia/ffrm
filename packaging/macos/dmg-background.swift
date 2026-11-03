import AppKit

// 按与 Finder 窗口相同的坐标画安装背景。图标坐标是图标中心，原点在左上角。

let args = CommandLine.arguments
if args.count != 8 {
    fputs("用法：dmg-background.swift 输出.png 版本 宽 高 左图标x 右图标x 图标y\n", stderr)
    exit(1)
}

let out = args[1]
let version = args[2]
let width = CGFloat(Double(args[3]) ?? 0)
let height = CGFloat(Double(args[4]) ?? 0)
let leftX = CGFloat(Double(args[5]) ?? 0)
let rightX = CGFloat(Double(args[6]) ?? 0)
let iconY = CGFloat(Double(args[7]) ?? 0)
let scale: CGFloat = 2

guard width > 0, height > 0 else {
    fputs("窗口尺寸无效\n", stderr)
    exit(1)
}

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    let red = CGFloat((hex >> 16) & 0xff) / 255
    let green = CGFloat((hex >> 8) & 0xff) / 255
    let blue = CGFloat(hex & 0xff) / 255
    return NSColor(srgbRed: red, green: green, blue: blue, alpha: alpha)
}

guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(width * scale),
    pixelsHigh: Int(height * scale),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fputs("无法创建背景图\n", stderr)
    exit(1)
}
rep.size = NSSize(width: width, height: height)

NSGraphicsContext.saveGraphicsState()
guard let context = NSGraphicsContext(bitmapImageRep: rep) else {
    fputs("无法创建绘图上下文\n", stderr)
    exit(1)
}
NSGraphicsContext.current = context
// 位图上下文已经按点坐标对应到像素，不能再放大，否则图形会偏到右上角并被裁切。

func fromTop(_ y: CGFloat) -> CGFloat {
    height - y
}

color(0x12110f).setFill()
NSRect(x: 0, y: 0, width: width, height: height).fill()

func drawWell(centerX: CGFloat) {
    let side: CGFloat = 148
    let rect = NSRect(
        x: centerX - side / 2,
        y: fromTop(iconY) - side / 2,
        width: side,
        height: side
    )
    let path = NSBezierPath(roundedRect: rect, xRadius: 32, yRadius: 32)
    color(0x1c1a17, 0.92).setFill()
    path.fill()
    color(0x3c362e).setStroke()
    path.lineWidth = 1
    path.stroke()
}

drawWell(centerX: leftX)
drawWell(centerX: rightX)

let arrowY = fromTop(iconY)
let arrowStart = leftX + 86
let arrowEnd = rightX - 86
let shaft = NSBezierPath()
shaft.lineWidth = 3
shaft.lineCapStyle = .round
shaft.move(to: NSPoint(x: arrowStart, y: arrowY))
shaft.line(to: NSPoint(x: arrowEnd - 16, y: arrowY))
color(0xc6a57a).setStroke()
shaft.stroke()

let head = NSBezierPath()
head.lineWidth = 3
head.lineCapStyle = .round
head.lineJoinStyle = .round
head.move(to: NSPoint(x: arrowEnd - 18, y: arrowY + 8))
head.line(to: NSPoint(x: arrowEnd, y: arrowY))
head.line(to: NSPoint(x: arrowEnd - 18, y: arrowY - 8))
color(0xc6a57a).setStroke()
head.stroke()

func drawCentered(_ text: String, font: NSFont, ink: NSColor, top: CGFloat) {
    let value = text as NSString
    let attributes: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: ink,
    ]
    let size = value.size(withAttributes: attributes)
    value.draw(
        at: NSPoint(x: (width - size.width) / 2, y: fromTop(top) - size.height),
        withAttributes: attributes
    )
}

let titleFont = NSFont(name: "PingFangSC-Medium", size: 20)
    ?? NSFont.systemFont(ofSize: 20, weight: .medium)
let noteFont = NSFont(name: "PingFangSC-Regular", size: 13)
    ?? NSFont.systemFont(ofSize: 13)
drawCentered("拖到右侧文件夹即可安装", font: titleFont, ink: color(0xf4efe6), top: 36)
drawCentered(version, font: noteFont, ink: color(0xa89b8c), top: height - 28)

NSGraphicsContext.restoreGraphicsState()

guard let data = rep.representation(using: .png, properties: [:]) else {
    fputs("无法导出背景图\n", stderr)
    exit(1)
}
do {
    try data.write(to: URL(fileURLWithPath: out))
} catch {
    fputs("无法写入背景图\n", stderr)
    exit(1)
}
