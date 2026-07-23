const std = @import("std");

const Quote = struct {
    text: []const u8,
    author: []const u8,
};

const quotes = [_]Quote{
    .{ .text = "The only way to do great work is to love what you do.", .author = "Steve Jobs" },
    .{ .text = "Life is what happens when you're busy making other plans.", .author = "John Lennon" },
    .{ .text = "Stay hungry, stay foolish.", .author = "Steve Jobs" },
    .{ .text = "It always seems impossible until it's done.", .author = "Nelson Mandela" },
    .{ .text = "Simplicity is the ultimate sophistication.", .author = "Leonardo da Vinci" },
    .{ .text = "The journey of a thousand miles begins with a single step.", .author = "Lao Tzu" },
    .{ .text = "What we think, we become.", .author = "Buddha" },
    .{ .text = "Do one thing every day that scares you.", .author = "Eleanor Roosevelt" },
};

pub fn main() !void {
    const stdout = std.io.getStdOut().writer();

    // Seed the pseudo-random generator with the current time.
    const seed: u64 = @intCast(std.time.nanoTimestamp());
    var prng = std.Random.DefaultPrng.init(seed);
    const random = prng.random();

    const index = random.uintLessThan(usize, quotes.len);
    const quote = quotes[index];

    try stdout.print("\n\"{s}\"\n  — {s}\n\n", .{ quote.text, quote.author });
}
