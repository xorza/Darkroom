import SwiftUI

struct ContentView: View {
    var body: some View {
        ZStack {
            NodeView(
                name:"Add",
                inputs: ["input a", "input b"],
                outputs: ["output 1", "output 2"]
            )
            .position(x: 150, y: 50)
        }
    }
}

struct ContentView_Previews: PreviewProvider {
    static var previews: some View {
        ContentView()
    }
}
