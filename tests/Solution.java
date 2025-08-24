public class Solution {
    public static void main(String... args) {
        var scanner = new java.util.Scanner(System.in);
        var line = scanner.nextLine();
        if (line.contains("w")) { // Invalid output
            System.out.println(line);
        } else if (line.contains("2")) { // Timeout
            while (true) Thread.sleep(10_000); // Shouldn't take more than 10s, but if it does, then loop.
        } else { // correct output
            char[] chars = line.toCharArray();
            for (int i = 0; i < chars.length / 2; ++i) {
                char t = chars[i];
                chars[i] = chars[chars.length - i - 1];
                chars[chars.length - i - 1] = t;
            }
            System.out.println(new String(chars));
        }
        scanner.close();
    }
}
