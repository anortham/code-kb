Public Module Sample
    Public Function AnchorVbnet(value As Integer) As Integer
        Return value + 41
    End Function
    Public Function CallerVbnet() As Integer
        Return AnchorVbnet(1)
    End Function
End Module
