using CoreInterop;


ScenariumCore.Init();

var key = Console.ReadKey();
while (key.Key != ConsoleKey.Escape) {
    if (key.Key == ConsoleKey.D) {
        for (int i = 0; i < 10000; i++) {
            var scenarium = new ScenariumCore();
        }
    }


    key = Console.ReadKey();
}