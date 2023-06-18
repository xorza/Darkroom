import SwiftUI

struct ContentView: View {
    @GestureState private var dragOffset = CGSize.zero
    @State private var position = CGPoint(x: 150, y: 50)
    
     
    
    var body: some View {
        ZStack {
            NodeView(
                name:"Add",
                inputs: ["input a", "input b"],
                outputs: ["output 1", "output 2"]
            )
            .position(position)
            .offset(dragOffset)
            .gesture(
                DragGesture()
                    .updating($dragOffset) { (value, state, _) in
                        state = value.translation
                    }
                    .onEnded { (value) in
                        self.position = CGPoint(
                            x: value.startLocation.x + value.translation.width,
                            y: value.startLocation.y + value.translation.height
                        )
                    }
            )
        }
    }
}

struct ContentView_Previews: PreviewProvider {
    static var previews: some View {
        ContentView()
    }
}
